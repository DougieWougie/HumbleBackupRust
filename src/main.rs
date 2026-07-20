mod api;
mod auth;
mod cache;
mod downloader;
mod naming;

use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use tokio::sync::Semaphore;

use api::{build_tasks, ApiError, HumbleClient, HumbleClientError};
use auth::firefox_session_cookie;
use downloader::{already_present, download_all, DownloadResult, DownloadTask, Status};

#[derive(Parser, Debug)]
#[command(name = "hbsync", about = "Sync Humble Bundle ebook purchases to local disk.")]
struct Cli {
    /// purchase keys or downloads?key=... URLs (default: whole library)
    keys: Vec<String>,

    /// destination directory (default: current directory)
    #[arg(short, long, default_value = ".")]
    output: PathBuf,

    /// comma-separated formats to download, e.g. epub,pdf (default: all)
    #[arg(long)]
    formats: Option<String>,

    /// number of concurrent downloads (default: 4)
    #[arg(long, default_value_t = 4)]
    parallel: usize,

    /// list what would be downloaded without downloading
    #[arg(long)]
    list: bool,

    /// Humble Bundle _simpleauth_sess cookie value (default: read from Firefox)
    #[arg(long)]
    cookie: Option<String>,

    /// bypass the order metadata cache and refetch everything from Humble Bundle
    #[arg(long)]
    refresh: bool,
}

fn parse_key(arg: &str) -> String {
    if let Some(idx) = arg.find("key=") {
        let rest = &arg[idx + 4..];
        let end = rest.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(rest.len());
        if end > 0 {
            return rest[..end].to_string();
        }
    }
    arg.to_string()
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let formats: Option<HashSet<String>> = cli.formats.as_ref().map(|s| {
        s.split(',')
            .map(|f| f.trim().to_lowercase())
            .filter(|f| !f.is_empty())
            .collect()
    });
    match run(cli, formats).await {
        Ok(code) => std::process::ExitCode::from(code),
        Err(err) => {
            eprintln!("{err}");
            std::process::ExitCode::from(1)
        }
    }
}

async fn run(cli: Cli, formats: Option<HashSet<String>>) -> Result<u8> {
    let cookie = match &cli.cookie {
        Some(c) => c.clone(),
        None => firefox_session_cookie(None)?,
    };
    let client = HumbleClient::new(&cookie)?;

    let keys: Vec<String> = if cli.keys.is_empty() {
        client.list_order_keys().await.map_err(map_client_err)?
    } else {
        cli.keys.iter().map(|k| parse_key(k)).collect()
    };

    let cache_dir = cache::cache_dir();
    let semaphore = Arc::new(Semaphore::new(cli.parallel.max(1)));
    let fetches = keys.iter().map(|key| {
        let client = &client;
        let semaphore = Arc::clone(&semaphore);
        let cache_dir = cache_dir.clone();
        let output = cli.output.clone();
        let formats = formats.clone();
        async move {
            let _permit = semaphore.acquire().await.expect("semaphore closed");
            let result = resolve_tasks(
                client,
                key,
                &output,
                formats.as_ref(),
                cache_dir.as_deref(),
                cli.refresh,
            )
            .await;
            (key.to_string(), result)
        }
    });
    let fetched = futures::future::join_all(fetches).await;

    let mut tasks = Vec::new();
    let mut key_failures: u32 = 0;
    for (key, result) in fetched {
        match result {
            Ok(order_tasks) => tasks.extend(order_tasks),
            Err(HumbleClientError::Api(ApiError::NotFound(msg))) => {
                eprintln!("\u{2717} {key}: not found: {msg}");
                key_failures += 1;
            }
            Err(HumbleClientError::Auth(err)) => return Err(err.into()),
            Err(HumbleClientError::Network(err)) => {
                return Err(anyhow::anyhow!("network error talking to Humble Bundle: {err}"));
            }
        }
    }

    let available = tasks.len();
    let already_downloaded = tasks.iter().filter(|t| already_present(t)).count();
    println!(
        "{available} available, {already_downloaded} already downloaded, {} to download",
        available - already_downloaded
    );

    if cli.list {
        for task in &tasks {
            println!("{}", task.dest.display());
        }
        println!("{} files", tasks.len());
        return Ok(if key_failures > 0 { 1 } else { 0 });
    }

    run_downloads(tasks, &cli, key_failures).await
}

/// Resolve the download tasks for one order key, preferring the cache.
///
/// A cache hit only avoids the network call when every task it produces is
/// already present on disk — otherwise the signed URLs it holds may have
/// expired, so a fresh fetch is needed anyway, and that fresh order is
/// written back to the cache.
async fn resolve_tasks(
    client: &HumbleClient,
    key: &str,
    output: &Path,
    formats: Option<&HashSet<String>>,
    cache_dir: Option<&Path>,
    refresh: bool,
) -> Result<Vec<DownloadTask>, HumbleClientError> {
    if !refresh {
        if let Some(dir) = cache_dir {
            if let Some(cached) = cache::load_order(dir, key) {
                let tasks = build_tasks(&cached, output, formats);
                if tasks.iter().all(already_present) {
                    return Ok(tasks);
                }
            }
        }
    }
    let order = client.get_order(key).await?;
    if let Some(dir) = cache_dir {
        cache::save_order(dir, key, &order);
    }
    Ok(build_tasks(&order, output, formats))
}

fn map_client_err(err: HumbleClientError) -> anyhow::Error {
    match err {
        HumbleClientError::Auth(e) => e.into(),
        HumbleClientError::Api(e) => e.into(),
        HumbleClientError::Network(e) => anyhow::anyhow!("network error talking to Humble Bundle: {e}"),
    }
}

async fn run_downloads(tasks: Vec<DownloadTask>, cli: &Cli, key_failures: u32) -> Result<u8> {
    let use_bar = std::io::stdout().is_terminal();
    let bar = use_bar.then(|| {
        let bar = indicatif::ProgressBar::new(tasks.len() as u64);
        bar.set_style(
            indicatif::ProgressStyle::with_template("[{bar:20.green/white}] {percent}% ({pos}/{len}){msg}")
                .expect("valid template")
                .progress_chars("█-"),
        );
        bar
    });
    let failed_count = AtomicU64::new(0);
    let on_result = |result: &DownloadResult| {
        if let Some(bar) = &bar {
            if matches!(result.status, Status::Failed(_)) {
                let n = failed_count.fetch_add(1, Ordering::SeqCst) + 1;
                bar.set_message(format!(" \u{b7} {n} failed"));
            }
            bar.inc(1);
        }
    };

    let http_client = reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?;
    let backoff = [Duration::from_secs(1), Duration::from_secs(2), Duration::from_secs(4)];
    let results = download_all(tasks, cli.parallel.max(1), http_client, &backoff, Some(&on_result)).await;
    if let Some(bar) = &bar {
        bar.finish();
        println!();
    }

    let mut downloaded = 0u32;
    let mut skipped = 0u32;
    let mut failed = 0u32;
    for result in &results {
        match &result.status {
            Status::Downloaded => downloaded += 1,
            Status::Skipped => skipped += 1,
            Status::Failed(err) => {
                failed += 1;
                println!("\u{2717} {} ({err})", result.task.dest.display());
            }
        }
    }
    println!("\n{downloaded} downloaded, {skipped} skipped, {failed} failed");
    Ok(if failed > 0 || key_failures > 0 { 1 } else { 0 })
}
