mod api;
mod auth;
mod downloader;
mod naming;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use tokio::sync::Semaphore;

use api::{build_tasks, ApiError, HumbleClient, HumbleClientError};
use auth::firefox_session_cookie;
use downloader::already_present;

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

    let semaphore = Arc::new(Semaphore::new(cli.parallel.max(1)));
    let fetches = keys.iter().cloned().map(|key| {
        let client = &client;
        let semaphore = Arc::clone(&semaphore);
        async move {
            let _permit = semaphore.acquire().await.expect("semaphore closed");
            (key.clone(), client.get_order(&key).await)
        }
    });
    let fetched = futures::future::join_all(fetches).await;

    let mut tasks = Vec::new();
    let mut key_failures: u32 = 0;
    for (key, result) in fetched {
        match result {
            Ok(order) => tasks.extend(build_tasks(&order, &cli.output, formats.as_ref())),
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

fn map_client_err(err: HumbleClientError) -> anyhow::Error {
    match err {
        HumbleClientError::Auth(e) => e.into(),
        HumbleClientError::Api(e) => e.into(),
        HumbleClientError::Network(e) => anyhow::anyhow!("network error talking to Humble Bundle: {e}"),
    }
}

// Placeholder — replaced in Task 9 with the real download/progress-bar path.
async fn run_downloads(_tasks: Vec<downloader::DownloadTask>, _cli: &Cli, key_failures: u32) -> Result<u8> {
    Ok(if key_failures > 0 { 1 } else { 0 })
}
