//! Parallel, resumable-safe file downloads with skip/verify semantics.

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use md5::{Digest, Md5};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};

use crate::api::is_loopback_url;

/// Buffer size for writing the download to disk and for re-hashing a partial
/// file on resume. Response chunks arrive at ~8-16KB; batching them keeps the
/// write syscall count proportional to file size / this, not / chunk size.
const IO_BUFFER: usize = 256 * 1024;

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
    /// The digest the order metadata claimed, when it differs from `md5`.
    /// Accepting a new digest means trusting the server over the recorded
    /// checksum, so callers should report it rather than swallow it.
    pub previous_md5: Option<String>,
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

/// Async twin of [`already_present`], for use inside download tasks so the
/// stat never blocks a runtime worker.
async fn already_present_async(task: &DownloadTask) -> bool {
    match tokio::fs::metadata(&task.dest).await {
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
    #[error("md5 mismatch: got {actual}, expected {expected}")]
    Md5Mismatch { actual: String, expected: String },
    #[error("refusing to download over an insecure URL: {0}")]
    InsecureUrl(String),
    #[error("server rejected the resume request; the partial file was discarded")]
    ResumeRejected,
}

impl FetchError {
    /// Whether retrying could plausibly succeed. Retrying a 404 or a
    /// server whose content genuinely differs from the recorded checksum
    /// just burns the backoff schedule.
    fn is_retryable(&self) -> bool {
        match self {
            FetchError::Http(err) => match err.status() {
                Some(status) => {
                    status.is_server_error()
                        || status == reqwest::StatusCode::REQUEST_TIMEOUT
                        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
                }
                // Connect, timeout, and mid-body transport errors.
                None => true,
            },
            // A truncated transfer is the classic transient failure.
            FetchError::SizeMismatch { .. } => true,
            FetchError::ResumeRejected => true,
            // Local disk problems (ENOSPC, EACCES) won't fix themselves,
            // and the remaining two are verdicts about the content itself.
            FetchError::Io(_) | FetchError::Md5Mismatch { .. } | FetchError::InsecureUrl(_) => false,
        }
    }
}

pub async fn download_all(
    tasks: Vec<DownloadTask>,
    parallel: usize,
    client: reqwest::Client,
    backoff: &[Duration],
    on_result: Option<&dyn Fn(&DownloadResult)>,
) -> Vec<DownloadResult> {
    let client = &client;
    futures::stream::iter(tasks.into_iter().map(|task| async move {
        let result = download_one(client, task, backoff).await;
        if let Some(cb) = on_result {
            cb(&result);
        }
        result
    }))
    // `buffered` keeps at most `parallel` downloads in flight while
    // preserving task order in the returned results.
    .buffered(parallel.max(1))
    .collect()
    .await
}

async fn download_one(
    client: &reqwest::Client,
    task: DownloadTask,
    backoff: &[Duration],
) -> DownloadResult {
    if already_present_async(&task).await {
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
            Err(err) => {
                let retryable = err.is_retryable();
                last_error = err.to_string();
                if !retryable {
                    break;
                }
            }
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

/// Only https is acceptable for download URLs, which arrive as data from the
/// order API. Plaintext loopback is allowed so the integration tests can
/// serve fixtures from a local mock server.
fn url_is_acceptable(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(parsed) => parsed.scheme() == "https" || is_loopback_url(url),
        Err(_) => false,
    }
}

/// Hash an existing `.part` file so a resumed download can continue the
/// same digest, returning the number of bytes hashed.
async fn hash_partial(path: &Path, hasher: &mut Md5) -> Result<u64, std::io::Error> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0u8; IO_BUFFER];
    let mut hashed = 0u64;
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            return Ok(hashed);
        }
        hasher.update(&buffer[..read]);
        hashed += read as u64;
    }
}

async fn fetch_inner(
    client: &reqwest::Client,
    task: &DownloadTask,
    part_path: &Path,
) -> Result<Option<CorrectedMetadata>, FetchError> {
    if !url_is_acceptable(&task.url) {
        return Err(FetchError::InsecureUrl(task.url.clone()));
    }

    // A `.part` left by an interrupted run is resumable: ask for the rest
    // rather than re-fetching bytes we already have.
    let resume_from = tokio::fs::metadata(part_path).await.map(|meta| meta.len()).unwrap_or(0);
    let mut request = client.get(&task.url);
    if resume_from > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={resume_from}-"));
    }
    let response = request.send().await?;
    if response.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        // The partial file is longer than the current remote file (or is
        // otherwise unusable). `fetch` discards it, so a retry starts clean.
        return Err(FetchError::ResumeRejected);
    }
    // A server that ignores `Range` answers 200 with the whole body, in which
    // case the partial file must be discarded rather than appended to.
    let resumed = resume_from > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let response = response.error_for_status()?;

    // Humble's order-detail API can report stale size/md5 for books that were
    // re-uploaded after purchase (revised editions, retranscodes). The
    // Content-Length of this exact response reflects what the server is
    // actually about to send, so it's the trustworthy check for truncation;
    // order metadata is only used as a fallback when Content-Length is absent.
    // On a 206 it covers only the remaining range, so add back what we hold.
    let content_length = response.content_length().map(|len| len + if resumed { resume_from } else { 0 });
    let metadata_stale = matches!((task.size, content_length), (Some(s), Some(c)) if s != c);

    let mut hasher = Md5::new();
    let mut actual_size: u64 = 0;
    let file = if resumed {
        actual_size = hash_partial(part_path, &mut hasher).await?;
        tokio::fs::OpenOptions::new().append(true).open(part_path).await?
    } else {
        tokio::fs::File::create(part_path).await?
    };
    let mut writer = BufWriter::with_capacity(IO_BUFFER, file);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        writer.write_all(&chunk).await?;
        hasher.update(&chunk);
        actual_size += chunk.len() as u64;
    }
    writer.flush().await?;
    drop(writer);

    let complete = match content_length.or(task.size) {
        Some(expected) if actual_size != expected => {
            return Err(FetchError::SizeMismatch { actual: actual_size, expected });
        }
        // Nothing to compare against: treat the stream ending as the only
        // available signal of completeness.
        expected => expected.is_some(),
    };

    let digest = format!("{:x}", hasher.finalize());
    let digest_differs = task.md5.as_ref().is_some_and(|expected| !expected.eq_ignore_ascii_case(&digest));
    // Only a verifiably complete response earns the right to overwrite the
    // recorded metadata; otherwise a truncated or substituted body would
    // install itself as the new expected checksum.
    let corrected = if metadata_stale && complete {
        Some(CorrectedMetadata {
            size: actual_size,
            md5: digest,
            previous_md5: digest_differs.then(|| task.md5.clone()).flatten(),
        })
    } else {
        if digest_differs {
            return Err(FetchError::Md5Mismatch {
                actual: digest,
                expected: task.md5.clone().unwrap_or_default(),
            });
        }
        None
    };
    tokio::fs::rename(part_path, &task.dest).await?;
    Ok(corrected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
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
    async fn resumes_from_an_existing_part_file() {
        let seen_ranges: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        struct RangeAware(Arc<Mutex<Vec<String>>>);
        impl Respond for RangeAware {
            fn respond(&self, req: &wiremock::Request) -> ResponseTemplate {
                match req.headers.get("range") {
                    Some(value) => {
                        let raw = value.to_str().unwrap().to_string();
                        let start: usize =
                            raw.trim_start_matches("bytes=").trim_end_matches('-').parse().unwrap();
                        self.0.lock().unwrap().push(raw);
                        ResponseTemplate::new(206)
                            .set_body_bytes(&CONTENT[start..])
                            .append_header(
                                "Content-Range",
                                format!("bytes {}-{}/{}", start, CONTENT.len() - 1, CONTENT.len())
                                    .as_str(),
                            )
                    }
                    None => ResponseTemplate::new(200).set_body_bytes(CONTENT),
                }
            }
        }
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/book.epub"))
            .respond_with(RangeAware(Arc::clone(&seen_ranges)))
            .mount(&server).await;

        let dir = tempdir().unwrap();
        let dest = dir.path().join("book.epub");
        // An interrupted earlier run left the first 6 bytes behind.
        std::fs::write(dir.path().join("book.epub.part"), &CONTENT[..6]).unwrap();
        let task = DownloadTask {
            url: format!("{}/book.epub", server.uri()),
            dest: dest.clone(),
            size: Some(CONTENT.len() as u64),
            md5: Some(content_md5()),
        };
        let results = download_all(vec![task], 1, reqwest::Client::new(), &[], None).await;

        assert_eq!(results[0].status, Status::Downloaded);
        assert_eq!(seen_ranges.lock().unwrap().as_slice(), ["bytes=6-"]);
        // The md5 covers the whole file, so a correct resume proves both that
        // the remainder was appended and that the existing bytes were hashed.
        assert_eq!(std::fs::read(&dest).unwrap(), CONTENT);
        assert!(!dir.path().join("book.epub.part").exists());
    }

    #[tokio::test]
    async fn ignored_range_header_restarts_cleanly() {
        // A server that answers 200 to a Range request is sending the whole
        // body; appending it to the partial file would corrupt the result.
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/book.epub"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(CONTENT))
            .mount(&server).await;
        let dir = tempdir().unwrap();
        let dest = dir.path().join("book.epub");
        std::fs::write(dir.path().join("book.epub.part"), &CONTENT[..6]).unwrap();
        let task = DownloadTask {
            url: format!("{}/book.epub", server.uri()),
            dest: dest.clone(),
            size: Some(CONTENT.len() as u64),
            md5: Some(content_md5()),
        };
        let results = download_all(vec![task], 1, reqwest::Client::new(), &[], None).await;
        assert_eq!(results[0].status, Status::Downloaded);
        assert_eq!(std::fs::read(&dest).unwrap(), CONTENT);
    }

    #[tokio::test]
    async fn non_retryable_status_is_not_retried() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        struct NotFound(Arc<AtomicUsize>);
        impl Respond for NotFound {
            fn respond(&self, _req: &wiremock::Request) -> ResponseTemplate {
                self.0.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(404)
            }
        }
        Mock::given(method("GET")).respond_with(NotFound(Arc::clone(&calls))).mount(&server).await;
        let dir = tempdir().unwrap();
        let task = DownloadTask {
            url: format!("{}/book.epub", server.uri()),
            dest: dir.path().join("book.epub"),
            size: None,
            md5: None,
        };
        let backoff = [Duration::ZERO, Duration::ZERO, Duration::ZERO];
        let results = download_all(vec![task], 1, reqwest::Client::new(), &backoff, None).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "a 404 must not be retried");
        assert!(matches!(results[0].status, Status::Failed(_)));
    }

    #[tokio::test]
    async fn md5_mismatch_is_not_retried() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        struct Counter(Arc<AtomicUsize>);
        impl Respond for Counter {
            fn respond(&self, _req: &wiremock::Request) -> ResponseTemplate {
                self.0.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_bytes(CONTENT)
            }
        }
        Mock::given(method("GET")).respond_with(Counter(Arc::clone(&calls))).mount(&server).await;
        let dir = tempdir().unwrap();
        let task = DownloadTask {
            url: format!("{}/book.epub", server.uri()),
            dest: dir.path().join("book.epub"),
            size: Some(CONTENT.len() as u64),
            md5: Some("0".repeat(32)),
        };
        let backoff = [Duration::ZERO, Duration::ZERO, Duration::ZERO];
        let results = download_all(vec![task], 1, reqwest::Client::new(), &backoff, None).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "content that is simply wrong won't change");
        assert!(matches!(results[0].status, Status::Failed(_)));
    }

    #[tokio::test]
    async fn plaintext_remote_url_is_refused_without_a_request() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("book.epub");
        let task = DownloadTask {
            url: "http://dl.humble.test/book.epub".to_string(),
            dest: dest.clone(),
            size: None,
            md5: None,
        };
        let results = download_all(vec![task], 1, reqwest::Client::new(), &[], None).await;
        match &results[0].status {
            Status::Failed(err) => assert!(err.contains("insecure"), "{err}"),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(!dest.exists());
    }

    #[test]
    fn url_acceptance_rules() {
        assert!(url_is_acceptable("https://dl.humble.test/book.epub"));
        assert!(url_is_acceptable("http://127.0.0.1:9000/book.epub"));
        assert!(!url_is_acceptable("http://dl.humble.test/book.epub"));
        assert!(!url_is_acceptable("file:///etc/passwd"));
        assert!(!url_is_acceptable("book.epub"));
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
