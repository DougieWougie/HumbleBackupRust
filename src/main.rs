mod api;
mod auth;
mod cache;
mod downloader;
mod naming;

use std::collections::HashSet;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use futures::StreamExt;

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

    /// Humble Bundle _simpleauth_sess cookie value. Visible to other local
    /// users via the process list; prefer HBSYNC_COOKIE or --cookie-stdin.
    #[arg(long)]
    cookie: Option<String>,

    /// read the _simpleauth_sess cookie value from standard input
    #[arg(long, conflicts_with = "cookie")]
    cookie_stdin: bool,

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

/// Resolve the session cookie: `--cookie-stdin`, then `--cookie`, then
/// `HBSYNC_COOKIE`, then the local Firefox profile.
///
/// `--cookie` is last among the explicit options for a reason — argv is
/// readable by every other user on the machine, so the stdin and environment
/// routes exist to keep the cookie out of the process list.
fn resolve_cookie(cli: &Cli) -> Result<String> {
    if cli.cookie_stdin {
        let mut value = String::new();
        std::io::stdin().read_to_string(&mut value).context("reading cookie from stdin")?;
        let value = value.trim().to_string();
        if value.is_empty() {
            anyhow::bail!("--cookie-stdin was given but stdin was empty");
        }
        return Ok(value);
    }
    if let Some(value) = &cli.cookie {
        return Ok(value.clone());
    }
    if let Some(value) = std::env::var("HBSYNC_COOKIE").ok().filter(|v| !v.trim().is_empty()) {
        return Ok(value.trim().to_string());
    }
    Ok(firefox_session_cookie(None)?)
}

/// The single HTTP client shared by metadata and download requests, so both
/// phases reuse one connection pool.
///
/// It deliberately sets no whole-request timeout: that would cap how long a
/// download may take in total, failing any large ebook on a slow link. The
/// read timeout bounds the gap between response chunks instead, which is
/// what actually detects a stalled transfer. API requests apply their own
/// total deadline per request (see `api::API_TIMEOUT`).
fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("hbsync/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .build()?)
}

async fn run(cli: Cli, formats: Option<HashSet<String>>) -> Result<u8> {
    let cookie = resolve_cookie(&cli)?;
    let http = http_client()?;
    let client = HumbleClient::new(http.clone(), &cookie);

    let keys: Vec<String> = if cli.keys.is_empty() {
        client.list_order_keys().await.map_err(map_client_err)?
    } else {
        cli.keys.iter().map(|k| parse_key(k)).collect()
    };

    let cache_dir = cache::cache_dir();
    let fetched: Vec<(String, _)> = futures::stream::iter(keys.iter().map(|key| {
        let client = &client;
        let cache_dir = cache_dir.as_deref();
        let output = &cli.output;
        let formats = formats.as_ref();
        async move {
            let result = resolve_tasks(client, key, output, formats, cache_dir, cli.refresh).await;
            (key.to_string(), result)
        }
    }))
    .buffered(cli.parallel.max(1))
    .collect()
    .await;

    let mut tasks = Vec::new();
    let mut orders: std::collections::HashMap<String, api::Order> = std::collections::HashMap::new();
    let mut key_failures: u32 = 0;
    for (key, result) in fetched {
        match result {
            Ok((order, order_tasks)) => {
                tasks.extend(order_tasks);
                orders.insert(key, order);
            }
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

    run_downloads(tasks, &cli, http, key_failures, orders, cache_dir).await
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
) -> Result<(api::Order, Vec<DownloadTask>), HumbleClientError> {
    if !refresh {
        if let Some(dir) = cache_dir {
            if let Some(cached) = cache::load_order(dir, key) {
                let tasks = build_tasks(&cached, output, formats);
                if tasks.iter().all(already_present) {
                    return Ok((cached, tasks));
                }
            }
        }
    }
    let order = client.get_order(key).await?;
    if let Some(dir) = cache_dir {
        cache::save_order(dir, key, &order);
    }
    let tasks = build_tasks(&order, output, formats);
    Ok((order, tasks))
}

fn map_client_err(err: HumbleClientError) -> anyhow::Error {
    match err {
        HumbleClientError::Auth(e) => e.into(),
        HumbleClientError::Api(e) => e.into(),
        HumbleClientError::Network(e) => anyhow::anyhow!("network error talking to Humble Bundle: {e}"),
    }
}

async fn run_downloads(
    tasks: Vec<DownloadTask>,
    cli: &Cli,
    http: reqwest::Client,
    key_failures: u32,
    mut orders: std::collections::HashMap<String, api::Order>,
    cache_dir: Option<PathBuf>,
) -> Result<u8> {
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
        if let Some(corrected) = &result.corrected {
            let detail = match &corrected.previous_md5 {
                Some(previous) => format!(
                    "; checksum changed from {previous} to {} — accepted because the response \
                     was complete",
                    corrected.md5
                ),
                None => "; corrected".to_string(),
            };
            let msg = format!(
                "\u{26a0} {}: order metadata disagreed with the live download{detail}",
                result.task.dest.display()
            );
            // Route through the bar so the warning prints above it instead of
            // colliding with its in-place redraw (which looks like a restart).
            match &bar {
                Some(bar) => bar.println(msg),
                None => eprintln!("{msg}"),
            }
        }
        if let Some(bar) = &bar {
            if matches!(result.status, Status::Failed(_)) {
                let n = failed_count.fetch_add(1, Ordering::SeqCst) + 1;
                bar.set_message(format!(" \u{b7} {n} failed"));
            }
            bar.inc(1);
        }
    };

    let backoff = [Duration::from_secs(1), Duration::from_secs(2), Duration::from_secs(4)];
    let results = download_all(tasks, cli.parallel.max(1), http, &backoff, Some(&on_result)).await;
    if let Some(bar) = &bar {
        bar.finish();
        println!();
    }

    apply_corrections(&results, &mut orders, cache_dir.as_deref());

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

/// Patch cached order metadata with the size/md5 actually observed for
/// downloads that disagreed with it, so a future run's `already_present`
/// check recognizes these files as complete instead of re-downloading them
/// forever (see downloader::CorrectedMetadata).
fn apply_corrections(
    results: &[DownloadResult],
    orders: &mut std::collections::HashMap<String, api::Order>,
    cache_dir: Option<&Path>,
) {
    let Some(cache_dir) = cache_dir else { return };
    let mut changed_keys: HashSet<String> = HashSet::new();
    for result in results {
        let Some(corrected) = &result.corrected else { continue };
        for (key, order) in orders.iter_mut() {
            let found = order
                .books
                .iter_mut()
                .flat_map(|book| book.files.iter_mut())
                .find(|file| file.url == result.task.url);
            if let Some(file) = found {
                file.size = Some(corrected.size);
                file.md5 = Some(corrected.md5.clone());
                changed_keys.insert(key.clone());
                break;
            }
        }
    }
    for key in changed_keys {
        if let Some(order) = orders.get(&key) {
            cache::save_order(cache_dir, &key, order);
        }
    }
}
