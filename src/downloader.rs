//! Parallel, resumable-safe file downloads with skip/verify semantics.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use md5::{Digest, Md5};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;

#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,
    pub dest: PathBuf,
    pub size: Option<u64>,
    pub md5: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Downloaded,
    Skipped,
    Failed(String),
}

/// The size/md5 actually observed for a download whose result disagreed
/// with the order metadata the task was built from. Callers that persist
/// order metadata (see `cache.rs`) should patch it with these values so
/// future runs recognize the file as already present.
#[derive(Debug, Clone)]
pub struct CorrectedMetadata {
    pub size: u64,
    pub md5: String,
}

#[derive(Debug, Clone)]
pub struct DownloadResult {
    pub task: DownloadTask,
    pub status: Status,
    pub corrected: Option<CorrectedMetadata>,
}

pub fn already_present(task: &DownloadTask) -> bool {
    match std::fs::metadata(&task.dest) {
        Ok(meta) => task.size.is_none_or(|size| meta.len() == size),
        Err(_) => false,
    }
}

#[derive(Debug, Error)]
enum FetchError {
    #[error("{0}")]
    Http(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("size mismatch: got {actual}, expected {expected}")]
    SizeMismatch { actual: u64, expected: u64 },
    #[error("md5 mismatch")]
    Md5Mismatch,
}

pub async fn download_all(
    tasks: Vec<DownloadTask>,
    parallel: usize,
    client: reqwest::Client,
    backoff: &[Duration],
    on_result: Option<&dyn Fn(&DownloadResult)>,
) -> Vec<DownloadResult> {
    let semaphore = Arc::new(Semaphore::new(parallel.max(1)));
    let futures_iter = tasks.into_iter().map(|task| {
        let semaphore = Arc::clone(&semaphore);
        let client = client.clone();
        async move {
            let _permit = semaphore.acquire().await.expect("semaphore closed");
            let result = download_one(&client, task, backoff).await;
            if let Some(cb) = on_result {
                cb(&result);
            }
            result
        }
    });
    futures::future::join_all(futures_iter).await
}

async fn download_one(
    client: &reqwest::Client,
    task: DownloadTask,
    backoff: &[Duration],
) -> DownloadResult {
    if already_present(&task) {
        return DownloadResult { task, status: Status::Skipped, corrected: None };
    }
    let mut last_error = "unknown error".to_string();
    for attempt in 0..=backoff.len() {
        if attempt > 0 {
            tokio::time::sleep(backoff[attempt - 1]).await;
        }
        match fetch(client, &task).await {
            Ok(corrected) => {
                return DownloadResult { task, status: Status::Downloaded, corrected };
            }
            Err(err) => last_error = err.to_string(),
        }
    }
    DownloadResult { task, status: Status::Failed(last_error), corrected: None }
}

fn part_path_for(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

async fn fetch(
    client: &reqwest::Client,
    task: &DownloadTask,
) -> Result<Option<CorrectedMetadata>, FetchError> {
    if let Some(parent) = task.dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let part_path = part_path_for(&task.dest);
    let result = fetch_inner(client, task, &part_path).await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&part_path).await;
    }
    result
}

async fn fetch_inner(
    client: &reqwest::Client,
    task: &DownloadTask,
    part_path: &Path,
) -> Result<Option<CorrectedMetadata>, FetchError> {
    let response = client.get(&task.url).send().await?.error_for_status()?;
    // Humble's order-detail API can report stale size/md5 for books that were
    // re-uploaded after purchase (revised editions, retranscodes). The
    // Content-Length of this exact response reflects what the server is
    // actually about to send, so it's the trustworthy check for truncation;
    // order metadata is only used as a fallback when Content-Length is absent.
    let content_length = response.content_length();
    let metadata_stale = matches!((task.size, content_length), (Some(s), Some(c)) if s != c);
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(part_path).await?;
    let mut hasher = Md5::new();
    let mut actual_size: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        hasher.update(&chunk);
        actual_size += chunk.len() as u64;
    }
    file.flush().await?;
    drop(file);

    if let Some(expected) = content_length.or(task.size) {
        if actual_size != expected {
            return Err(FetchError::SizeMismatch { actual: actual_size, expected });
        }
    }
    let digest = format!("{:x}", hasher.finalize());
    let corrected = if metadata_stale {
        eprintln!(
            "\u{26a0} {}: order metadata disagrees with the live download; caching corrected size/md5",
            task.dest.display()
        );
        Some(CorrectedMetadata { size: actual_size, md5: digest })
    } else {
        if let Some(expected_md5) = &task.md5 {
            if &digest != expected_md5 {
                return Err(FetchError::Md5Mismatch);
            }
        }
        None
    };
    tokio::fs::rename(part_path, &task.dest).await?;
    Ok(corrected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn already_present_missing_file() {
        let dir = tempdir().unwrap();
        let task = DownloadTask {
            url: "https://dl.test/a.epub".to_string(),
            dest: dir.path().join("a.epub"),
            size: Some(10),
            md5: None,
        };
        assert!(!already_present(&task));
    }

    #[test]
    fn already_present_matching_size() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("a.epub");
        std::fs::write(&dest, b"1234567890").unwrap();
        let task = DownloadTask { url: "https://dl.test/a.epub".to_string(), dest, size: Some(10), md5: None };
        assert!(already_present(&task));
    }

    #[test]
    fn already_present_mismatched_size() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("a.epub");
        std::fs::write(&dest, b"123").unwrap();
        let task = DownloadTask { url: "https://dl.test/a.epub".to_string(), dest, size: Some(10), md5: None };
        assert!(!already_present(&task));
    }

    #[test]
    fn already_present_no_size_check() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("a.epub");
        std::fs::write(&dest, b"anything").unwrap();
        let task = DownloadTask { url: "https://dl.test/a.epub".to_string(), dest, size: None, md5: None };
        assert!(already_present(&task));
    }

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

    const CONTENT: &[u8] = b"hello ebook world";

    fn content_md5() -> String {
        let mut hasher = Md5::new();
        hasher.update(CONTENT);
        format!("{:x}", hasher.finalize())
    }

    #[tokio::test]
    async fn downloads_file_and_removes_part() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/book.epub"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(CONTENT))
            .mount(&server).await;
        let dir = tempdir().unwrap();
        let dest = dir.path().join("a").join("book.epub");
        let task = DownloadTask {
            url: format!("{}/book.epub", server.uri()),
            dest: dest.clone(),
            size: Some(CONTENT.len() as u64),
            md5: Some(content_md5()),
        };
        let client = reqwest::Client::new();
        let results = download_all(vec![task], 1, client, &[], None).await;
        assert_eq!(results[0].status, Status::Downloaded);
        assert_eq!(std::fs::read(&dest).unwrap(), CONTENT);
        assert!(!dir.path().join("a").join("book.epub.part").exists());
    }

    #[tokio::test]
    async fn skips_existing_file_with_matching_size() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        struct Counter(Arc<AtomicUsize>);
        impl Respond for Counter {
            fn respond(&self, _req: &wiremock::Request) -> ResponseTemplate {
                self.0.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_bytes(CONTENT)
            }
        }
        Mock::given(method("GET")).and(path("/book.epub"))
            .respond_with(Counter(Arc::clone(&calls)))
            .mount(&server).await;
        let dir = tempdir().unwrap();
        let dest = dir.path().join("book.epub");
        std::fs::write(&dest, CONTENT).unwrap();
        let task = DownloadTask {
            url: format!("{}/book.epub", server.uri()),
            dest,
            size: Some(CONTENT.len() as u64),
            md5: Some(content_md5()),
        };
        let client = reqwest::Client::new();
        let results = download_all(vec![task], 1, client, &[], None).await;
        assert_eq!(results[0].status, Status::Skipped);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn redownloads_file_with_wrong_size() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/book.epub"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(CONTENT))
            .mount(&server).await;
        let dir = tempdir().unwrap();
        let dest = dir.path().join("book.epub");
        std::fs::write(&dest, b"partial").unwrap();
        let task = DownloadTask {
            url: format!("{}/book.epub", server.uri()),
            dest: dest.clone(),
            size: Some(CONTENT.len() as u64),
            md5: Some(content_md5()),
        };
        let client = reqwest::Client::new();
        let results = download_all(vec![task], 1, client, &[], None).await;
        assert_eq!(results[0].status, Status::Downloaded);
        assert_eq!(std::fs::read(&dest).unwrap(), CONTENT);
    }

    #[tokio::test]
    async fn md5_mismatch_fails_and_leaves_no_file() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/book.epub"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(CONTENT))
            .mount(&server).await;
        let dir = tempdir().unwrap();
        let dest = dir.path().join("book.epub");
        let task = DownloadTask {
            url: format!("{}/book.epub", server.uri()),
            dest: dest.clone(),
            size: Some(CONTENT.len() as u64),
            md5: Some("0".repeat(32)),
        };
        let client = reqwest::Client::new();
        let results = download_all(vec![task], 1, client, &[], None).await;
        match &results[0].status {
            Status::Failed(err) => assert!(err.to_lowercase().contains("md5")),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(!dest.exists());
        assert!(!dir.path().join("book.epub.part").exists());
    }

    #[tokio::test]
    async fn stale_order_metadata_does_not_fail_a_complete_download() {
        // Humble's order-detail API can report a stale size/md5 for a book
        // that was re-uploaded after purchase. The live Content-Length of
        // the response should win over that stale metadata.
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/book.epub"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(CONTENT))
            .mount(&server).await;
        let dir = tempdir().unwrap();
        let dest = dir.path().join("book.epub");
        let task = DownloadTask {
            url: format!("{}/book.epub", server.uri()),
            dest: dest.clone(),
            size: Some(CONTENT.len() as u64 + 1000),
            md5: Some("0".repeat(32)),
        };
        let client = reqwest::Client::new();
        let results = download_all(vec![task], 1, client, &[], None).await;
        assert_eq!(results[0].status, Status::Downloaded);
        assert_eq!(std::fs::read(&dest).unwrap(), CONTENT);
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        struct Flaky(Arc<AtomicUsize>);
        impl Respond for Flaky {
            fn respond(&self, _req: &wiremock::Request) -> ResponseTemplate {
                let n = self.0.fetch_add(1, Ordering::SeqCst);
                if n == 0 { ResponseTemplate::new(500) } else { ResponseTemplate::new(200).set_body_bytes(CONTENT) }
            }
        }
        Mock::given(method("GET")).and(path("/book.epub"))
            .respond_with(Flaky(Arc::clone(&calls)))
            .mount(&server).await;
        let dir = tempdir().unwrap();
        let task = DownloadTask {
            url: format!("{}/book.epub", server.uri()),
            dest: dir.path().join("book.epub"),
            size: Some(CONTENT.len() as u64),
            md5: Some(content_md5()),
        };
        let client = reqwest::Client::new();
        let backoff = [Duration::ZERO, Duration::ZERO, Duration::ZERO];
        let results = download_all(vec![task], 1, client, &backoff, None).await;
        assert_eq!(results[0].status, Status::Downloaded);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn exhausted_retries_reports_failure() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        struct AlwaysFail(Arc<AtomicUsize>);
        impl Respond for AlwaysFail {
            fn respond(&self, _req: &wiremock::Request) -> ResponseTemplate {
                self.0.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(503)
            }
        }
        Mock::given(method("GET")).and(path("/book.epub"))
            .respond_with(AlwaysFail(Arc::clone(&calls)))
            .mount(&server).await;
        let dir = tempdir().unwrap();
        let task = DownloadTask {
            url: format!("{}/book.epub", server.uri()),
            dest: dir.path().join("book.epub"),
            size: None,
            md5: None,
        };
        let client = reqwest::Client::new();
        let backoff = [Duration::ZERO, Duration::ZERO, Duration::ZERO];
        let results = download_all(vec![task], 1, client, &backoff, None).await;
        assert_eq!(calls.load(Ordering::SeqCst), 4); // initial + 3 retries
        match &results[0].status {
            Status::Failed(err) => assert!(err.contains("503")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn on_result_callback_fires_per_task() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(CONTENT))
            .mount(&server).await;
        let dir = tempdir().unwrap();
        let tasks: Vec<DownloadTask> = (0..3)
            .map(|i| DownloadTask {
                url: format!("{}/{}.epub", server.uri(), i),
                dest: dir.path().join(format!("{i}.epub")),
                size: None,
                md5: None,
            })
            .collect();
        let expected_urls: Vec<String> = tasks.iter().map(|t| t.url.clone()).collect();
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_cb = Arc::clone(&seen);
        let on_result = move |result: &DownloadResult| {
            seen_cb.lock().unwrap().push(result.task.url.clone());
        };
        let client = reqwest::Client::new();
        let results = download_all(tasks.clone(), 3, client, &[], Some(&on_result)).await;
        let result_urls: Vec<String> = results.iter().map(|r| r.task.url.clone()).collect();
        assert_eq!(result_urls, expected_urls); // results keep task order
        let mut seen_sorted = seen.lock().unwrap().clone();
        seen_sorted.sort();
        let mut expected_sorted = expected_urls.clone();
        expected_sorted.sort();
        assert_eq!(seen_sorted, expected_sorted);
    }
}
