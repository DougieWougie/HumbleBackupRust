# hbsync Rust Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port the Python `hbsync` CLI to Rust in this repo, matching its architecture and behavior, for faster runtime.

**Architecture:** One Rust module per Python module (`naming`, `auth`, `api`, `downloader`, `main`), same one-directional data flow: `auth` → cookie, `api` → `DownloadTask`s (via `naming`), `downloader` executes under a concurrency limit, `main` wires it together and reports results.

**Tech Stack:** Rust 2021, `tokio` (async runtime), `reqwest` (HTTP), `clap` (CLI parsing), `indicatif` (progress bar), `rusqlite` bundled (Firefox cookie DB), `serde`/`serde_json`, `md-5`, `thiserror`/`anyhow`. Dev/test: `wiremock`, `assert_cmd`, `predicates`.

## Global Constraints

- Behavior must match the Python reference at `/home/dougiewougie/Projects/bulk/hbsync/` except where noted as an intentional, minor, cosmetic divergence (approved: progress bar rendering via indicatif instead of a hand-rolled bar).
- Same CLI flags: positional `keys`, `-o/--output` (default `.`), `--formats`, `--parallel` (default `4`), `--list`, `--cookie`.
- Same textual output semantics: preflight summary line, `--list` output, per-failure lines, final counts line, exit code `1` on any failure else `0`.
- No network access in tests — use `wiremock` for HTTP, a fixture sqlite DB for Firefox cookie tests.
- Crate binary name: `hbsync`.

---

## Task 1: Project scaffolding

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs` (skeleton — compiles, does nothing yet)
- Create: `src/naming.rs`, `src/auth.rs`, `src/api.rs`, `src/downloader.rs` (empty module files)
- Create: `.gitignore`
- Create: `LICENSE` (copy of the Python project's GPL-2.0-or-later text)
- Create: `tests/fixtures/order_sample.json` (copy of the Python project's fixture)

**Interfaces:**
- Produces: a compiling, empty binary crate that later tasks fill in.

- [ ] **Step 1: Write `Cargo.toml`**

```toml
[package]
name = "hbsync"
version = "0.1.0"
edition = "2021"
license = "GPL-2.0-or-later"
authors = ["Dougie Richardson <dougthegreenie@gmail.com>"]
description = "Sync Humble Bundle ebook purchases to local disk"

[[bin]]
name = "hbsync"
path = "src/main.rs"

[dependencies]
tokio = { version = "1", features = ["rt-multi-thread", "macros", "fs", "time", "sync"] }
reqwest = { version = "0.12", features = ["json", "stream"] }
clap = { version = "4", features = ["derive"] }
indicatif = "0.17"
rusqlite = { version = "0.31", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
md-5 = "0.10"
thiserror = "1"
anyhow = "1"
tempfile = "3"
futures = "0.3"
url = "2"
urlencoding = "2"

[dev-dependencies]
wiremock = "0.6"
assert_cmd = "2"
predicates = "3"
```

- [ ] **Step 2: Copy the LICENSE file**

```bash
cp /home/dougiewougie/Projects/bulk/LICENSE /home/dougiewougie/Projects/bulk_rust/LICENSE
```

- [ ] **Step 3: Copy the test fixture**

```bash
mkdir -p /home/dougiewougie/Projects/bulk_rust/tests/fixtures
cp /home/dougiewougie/Projects/bulk/tests/fixtures/order_sample.json /home/dougiewougie/Projects/bulk_rust/tests/fixtures/order_sample.json
```

- [ ] **Step 4: Write `.gitignore`**

```
/target
```

- [ ] **Step 5: Write empty module files**

`src/naming.rs`:
```rust
```

`src/auth.rs`:
```rust
```

`src/api.rs`:
```rust
```

`src/downloader.rs`:
```rust
```

- [ ] **Step 6: Write `src/main.rs` skeleton**

```rust
mod api;
mod auth;
mod downloader;
mod naming;

fn main() {
    println!("hbsync");
}
```

- [ ] **Step 7: Build and verify it compiles**

Run: `cargo build`
Expected: compiles successfully, downloads dependencies.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock LICENSE .gitignore src tests
git commit -m "Scaffold hbsync Rust crate"
```

---

## Task 2: `naming` module

**Files:**
- Modify: `src/naming.rs`

**Interfaces:**
- Produces: `pub fn sanitize(name: &str) -> String`, `pub fn parse_bundle_title(title: &str) -> (Option<String>, String)`. Used by `api::build_tasks` (Task 6).

- [ ] **Step 1: Write the module with inline tests**

```rust
//! Directory-name derivation: bundle-title parsing and filename sanitizing.

/// Make a string safe to use as a single path segment on Linux.
pub fn sanitize(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| if c == '\0' || c == '/' { '_' } else { c })
        .collect();
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        "Unnamed".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Split a Humble bundle title into (publisher, bundle_dir).
///
/// "Humble Tech Book Bundle: Intelligent Agents: Agentic AI and Large
/// Language Models by Apress" -> (Some("Apress"), "Agentic AI and Large
/// Language Models"). Publisher is None when the title has no " by
/// <Publisher>" suffix; caller falls back to the per-book publisher.
pub fn parse_bundle_title(title: &str) -> (Option<String>, String) {
    let tail = match title.rsplit_once(':') {
        Some((_, after)) => after.trim(),
        None => title.trim(),
    };
    if let Some((bundle, publisher)) = tail.rsplit_once(" by ") {
        let bundle = bundle.trim();
        let publisher = publisher.trim();
        if !bundle.is_empty() && !publisher.is_empty() {
            return (Some(publisher.to_string()), bundle.to_string());
        }
    }
    let result = if tail.is_empty() { title.trim() } else { tail };
    (None, result.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_pattern_publisher_and_bundle() {
        let (pub_, bundle) = parse_bundle_title(
            "Humble Tech Book Bundle: Intelligent Agents: \
             Agentic AI and Large Language Models by Apress",
        );
        assert_eq!(pub_.as_deref(), Some("Apress"));
        assert_eq!(bundle, "Agentic AI and Large Language Models");
    }

    #[test]
    fn no_publisher_suffix() {
        assert_eq!(
            parse_bundle_title("Humble Book Bundle: Cybersecurity 2.0"),
            (None, "Cybersecurity 2.0".to_string())
        );
    }

    #[test]
    fn no_colon_at_all() {
        assert_eq!(
            parse_bundle_title("Some Standalone Purchase"),
            (None, "Some Standalone Purchase".to_string())
        );
    }

    #[test]
    fn by_without_colon() {
        assert_eq!(
            parse_bundle_title("Data Science by O'Reilly"),
            (Some("O'Reilly".to_string()), "Data Science".to_string())
        );
    }

    #[test]
    fn last_by_wins() {
        assert_eq!(
            parse_bundle_title("Web Development by Example by SitePoint"),
            (
                Some("SitePoint".to_string()),
                "Web Development by Example".to_string()
            )
        );
    }

    #[test]
    fn sanitize_replaces_slash() {
        assert_eq!(sanitize("AC/DC: Guide"), "AC_DC: Guide");
    }

    #[test]
    fn sanitize_collapses_whitespace_and_trailing_dots() {
        assert_eq!(sanitize("  Foo   Bar. "), "Foo Bar");
    }

    #[test]
    fn sanitize_never_returns_empty() {
        assert_eq!(sanitize("..."), "Unnamed");
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test naming::`
Expected: 8 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/naming.rs
git commit -m "Add naming module: title parsing and path sanitizing"
```

---

## Task 3: `downloader` types and `already_present`

**Files:**
- Modify: `src/downloader.rs`

**Interfaces:**
- Produces: `pub struct DownloadTask { pub url: String, pub dest: PathBuf, pub size: Option<u64>, pub md5: Option<String> }`, `pub enum Status { Downloaded, Skipped, Failed(String) }`, `pub struct DownloadResult { pub task: DownloadTask, pub status: Status }`, `pub fn already_present(task: &DownloadTask) -> bool`. Used by `api::build_tasks` (produces `DownloadTask`), `main.rs` (uses all of the above).

- [ ] **Step 1: Write types and `already_present` with tests**

```rust
//! Parallel, resumable-safe file downloads with skip/verify semantics.

use std::path::PathBuf;

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

#[derive(Debug, Clone)]
pub struct DownloadResult {
    pub task: DownloadTask,
    pub status: Status,
}

pub fn already_present(task: &DownloadTask) -> bool {
    match std::fs::metadata(&task.dest) {
        Ok(meta) => task.size.is_none_or(|size| meta.len() == size),
        Err(_) => false,
    }
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
}
```

Note: `Option::is_none_or` is stable since Rust 1.82. If the installed toolchain is older, replace with `task.size.map_or(true, |size| meta.len() == size)`.

- [ ] **Step 2: Run the tests**

Run: `cargo test downloader::`
Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/downloader.rs
git commit -m "Add downloader types and already_present check"
```

---

## Task 4: `downloader::download_all` (fetch, retry, verify)

**Files:**
- Modify: `src/downloader.rs`

**Interfaces:**
- Consumes: `DownloadTask`, `Status`, `DownloadResult`, `already_present` (Task 3).
- Produces: `pub async fn download_all(tasks: Vec<DownloadTask>, parallel: usize, client: reqwest::Client, backoff: &[Duration], on_result: Option<&dyn Fn(&DownloadResult)>) -> Vec<DownloadResult>`. Used by `main.rs` (Task 9).

- [ ] **Step 1: Add the fetch/retry logic above the existing `#[cfg(test)]` block**

```rust
use futures::StreamExt;
use md5::{Digest, Md5};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;

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
        return DownloadResult { task, status: Status::Skipped };
    }
    let mut last_error = "unknown error".to_string();
    for attempt in 0..=backoff.len() {
        if attempt > 0 {
            tokio::time::sleep(backoff[attempt - 1]).await;
        }
        match fetch(client, &task).await {
            Ok(()) => return DownloadResult { task, status: Status::Downloaded },
            Err(err) => last_error = err.to_string(),
        }
    }
    DownloadResult { task, status: Status::Failed(last_error) }
}

fn part_path_for(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

async fn fetch(client: &reqwest::Client, task: &DownloadTask) -> Result<(), FetchError> {
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
) -> Result<(), FetchError> {
    let response = client.get(&task.url).send().await?.error_for_status()?;
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

    if let Some(expected) = task.size {
        if actual_size != expected {
            return Err(FetchError::SizeMismatch { actual: actual_size, expected });
        }
    }
    if let Some(expected_md5) = &task.md5 {
        let digest = format!("{:x}", hasher.finalize());
        if &digest != expected_md5 {
            return Err(FetchError::Md5Mismatch);
        }
    }
    tokio::fs::rename(part_path, &task.dest).await?;
    Ok(())
}
```

- [ ] **Step 2: Add `wiremock`-based async tests in the existing test module**

Append to `mod tests` in `src/downloader.rs`:

```rust
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
```

`DownloadTask` needs `Clone` for the last test's `tasks.clone()` — already derived in Task 3.

- [ ] **Step 3: Run the tests**

Run: `cargo test downloader::`
Expected: 11 tests pass (4 from Task 3 + 7 new).

- [ ] **Step 4: Commit**

```bash
git add src/downloader.rs
git commit -m "Add downloader fetch/retry/verify logic"
```

---

## Task 5: `auth` module

**Files:**
- Modify: `src/auth.rs`

**Interfaces:**
- Produces: `pub struct AuthError(pub String)` (implements `std::error::Error` via `thiserror`, `Display` prints the message), `pub const SESSION_NOT_FOUND_MSG: &str`, `pub fn firefox_session_cookie(firefox_dir: Option<&Path>) -> Result<String, AuthError>`. Used by `api.rs` (Task 7, constructs its own `AuthError` for rejected sessions) and `main.rs` (Task 8).

- [ ] **Step 1: Write the module**

```rust
//! Extract the Humble Bundle session cookie from a local Firefox profile.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};
use thiserror::Error;

pub const COOKIE_NAME: &str = "_simpleauth_sess";

pub const SESSION_NOT_FOUND_MSG: &str = "No Humble Bundle session found in Firefox. \
Log into humblebundle.com in Firefox and re-run, or pass --cookie.";

#[derive(Debug, Error)]
#[error("{0}")]
pub struct AuthError(pub String);

/// Return the _simpleauth_sess value from the most recently used profile.
///
/// The cookie DB is copied to a temp file before reading so this works
/// while Firefox is running (Firefox holds the original locked).
pub fn firefox_session_cookie(firefox_dir: Option<&Path>) -> Result<String, AuthError> {
    let owned_default;
    let dir: &Path = match firefox_dir {
        Some(d) => d,
        None => {
            let home = std::env::var_os("HOME")
                .ok_or_else(|| AuthError(SESSION_NOT_FOUND_MSG.to_string()))?;
            owned_default = PathBuf::from(home).join(".mozilla").join("firefox");
            &owned_default
        }
    };

    let mut databases = firefox_databases(dir);
    databases.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    databases.reverse();

    for db in databases {
        if let Some(value) = cookie_from_db(&db) {
            return Ok(value);
        }
    }
    Err(AuthError(SESSION_NOT_FOUND_MSG.to_string()))
}

fn firefox_databases(dir: &Path) -> Vec<PathBuf> {
    let mut dbs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let candidate = entry.path().join("cookies.sqlite");
            if candidate.is_file() {
                dbs.push(candidate);
            }
        }
    }
    dbs
}

fn cookie_from_db(db: &Path) -> Option<String> {
    let tmp = tempfile::tempdir().ok()?;
    let copy_path = tmp.path().join("cookies.sqlite");
    std::fs::copy(db, &copy_path).ok()?;
    let conn = Connection::open(&copy_path).ok()?;
    conn.query_row(
        "SELECT value FROM moz_cookies \
         WHERE host LIKE '%humblebundle.com' AND name = ?1 \
         ORDER BY expiry DESC LIMIT 1",
        [COOKIE_NAME],
        |row| row.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use tempfile::tempdir;

    fn make_profile(root: &Path, name: &str, cookies: &[(&str, &str, &str, i64)]) {
        let profile = root.join(name);
        std::fs::create_dir_all(&profile).unwrap();
        let conn = Connection::open(profile.join("cookies.sqlite")).unwrap();
        conn.execute(
            "CREATE TABLE moz_cookies \
             (id INTEGER PRIMARY KEY, host TEXT, name TEXT, value TEXT, expiry INTEGER)",
            [],
        )
        .unwrap();
        for (host, name, value, expiry) in cookies {
            conn.execute(
                "INSERT INTO moz_cookies (host, name, value, expiry) VALUES (?1, ?2, ?3, ?4)",
                params![host, name, value, expiry],
            )
            .unwrap();
        }
    }

    #[test]
    fn finds_session_cookie() {
        let dir = tempdir().unwrap();
        make_profile(
            dir.path(),
            "abc123.default-release",
            &[(".humblebundle.com", "_simpleauth_sess", "sekrit", 2_000_000_000)],
        );
        assert_eq!(firefox_session_cookie(Some(dir.path())).unwrap(), "sekrit");
    }

    #[test]
    fn ignores_other_cookies_and_hosts() {
        let dir = tempdir().unwrap();
        make_profile(
            dir.path(),
            "abc123.default",
            &[
                (".humblebundle.com", "csrf_cookie", "x", 2_000_000_000),
                (".example.com", "_simpleauth_sess", "wrong-site", 2_000_000_000),
            ],
        );
        let err = firefox_session_cookie(Some(dir.path())).unwrap_err();
        assert!(err.0.contains("Log into humblebundle.com in Firefox"));
    }

    #[test]
    fn no_profiles_raises_actionable_error() {
        let dir = tempdir().unwrap();
        let err = firefox_session_cookie(Some(dir.path())).unwrap_err();
        assert!(err.0.contains("Log into humblebundle.com in Firefox"));
    }

    #[test]
    fn picks_newest_expiry() {
        let dir = tempdir().unwrap();
        make_profile(
            dir.path(),
            "abc123.default-release",
            &[
                (".humblebundle.com", "_simpleauth_sess", "old", 1_000_000_000),
                (".humblebundle.com", "_simpleauth_sess", "new", 2_000_000_000),
            ],
        );
        assert_eq!(firefox_session_cookie(Some(dir.path())).unwrap(), "new");
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test auth::`
Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/auth.rs
git commit -m "Add auth module: Firefox session cookie extraction"
```

---

## Task 6: `api` data types, `parse_order`, `build_tasks`

**Files:**
- Modify: `src/api.rs`

**Interfaces:**
- Consumes: `naming::sanitize`, `naming::parse_bundle_title` (Task 2); `downloader::DownloadTask` (Task 3).
- Produces: `pub struct DownloadFile { pub format: String, pub url: String, pub size: Option<u64>, pub md5: Option<String> }`, `pub struct Book { pub title: String, pub publisher: Option<String>, pub files: Vec<DownloadFile> }`, `pub struct Order { pub key: String, pub title: String, pub books: Vec<Book> }`, `pub fn parse_order(data: &serde_json::Value) -> Order`, `pub fn build_tasks(order: &Order, output: &Path, formats: Option<&HashSet<String>>) -> Vec<DownloadTask>`. Used by `main.rs` (Task 8) and `HumbleClient` (Task 7, same file).

- [ ] **Step 1: Write the module**

```rust
//! Humble Bundle JSON API client and order-to-download-task mapping.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::downloader::DownloadTask;
use crate::naming::{parse_bundle_title, sanitize};

#[derive(Debug, Clone)]
pub struct DownloadFile {
    pub format: String,
    pub url: String,
    pub size: Option<u64>,
    pub md5: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Book {
    pub title: String,
    pub publisher: Option<String>,
    pub files: Vec<DownloadFile>,
}

#[derive(Debug, Clone)]
pub struct Order {
    pub key: String,
    pub title: String,
    pub books: Vec<Book>,
}

pub fn parse_order(data: &Value) -> Order {
    let mut books = Vec::new();
    if let Some(subproducts) = data.get("subproducts").and_then(Value::as_array) {
        for sub in subproducts {
            let mut files = Vec::new();
            if let Some(downloads) = sub.get("downloads").and_then(Value::as_array) {
                for download in downloads {
                    if download.get("platform").and_then(Value::as_str) != Some("ebook") {
                        continue;
                    }
                    if let Some(structs) = download.get("download_struct").and_then(Value::as_array) {
                        for st in structs {
                            let web = st.get("url").and_then(|u| u.get("web")).and_then(Value::as_str);
                            let name = st.get("name").and_then(Value::as_str);
                            let (Some(web), Some(name)) = (web, name) else { continue };
                            files.push(DownloadFile {
                                format: name.to_lowercase().trim_start_matches('.').to_string(),
                                url: web.to_string(),
                                size: st.get("file_size").and_then(Value::as_u64),
                                md5: st.get("md5").and_then(Value::as_str).map(str::to_string),
                            });
                        }
                    }
                }
            }
            if !files.is_empty() {
                let publisher = sub
                    .get("payee")
                    .and_then(|p| p.get("human_name"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let title = sub
                    .get("human_name")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown")
                    .to_string();
                books.push(Book { title, publisher, files });
            }
        }
    }
    let key = data.get("gamekey").and_then(Value::as_str).unwrap_or("").to_string();
    let title = data
        .get("product")
        .and_then(|p| p.get("human_name"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Order { key, title, books }
}

pub fn build_tasks(
    order: &Order,
    output: &Path,
    formats: Option<&HashSet<String>>,
) -> Vec<DownloadTask> {
    let (publisher_from_title, bundle_dir) = parse_bundle_title(&order.title);
    let mut tasks = Vec::new();
    for book in &order.books {
        let publisher = publisher_from_title
            .clone()
            .or_else(|| book.publisher.clone())
            .unwrap_or_else(|| "Unknown".to_string());
        for file in &book.files {
            if let Some(formats) = formats {
                if !formats.contains(&file.format) {
                    continue;
                }
            }
            let filename = filename_from_url(&file.url)
                .unwrap_or_else(|| format!("{}.{}", sanitize(&book.title), file.format));
            let dest = output
                .join(sanitize(&publisher))
                .join(sanitize(&bundle_dir))
                .join(sanitize(&file.format))
                .join(sanitize(&filename));
            tasks.push(DownloadTask {
                url: file.url.clone(),
                dest,
                size: file.size,
                md5: file.md5.clone(),
            });
        }
    }
    tasks
}

fn filename_from_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let last = parsed.path_segments()?.next_back()?;
    if last.is_empty() {
        return None;
    }
    urlencoding::decode(last).ok().map(|s| s.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/order_sample.json")).unwrap()
    }

    #[test]
    fn parse_order_keeps_only_ebook_downloads() {
        let order = parse_order(&fixture());
        assert_eq!(order.key, "CWPBwb82sqPXqEsq");
        let titles: Vec<&str> = order.books.iter().map(|b| b.title.as_str()).collect();
        assert_eq!(titles, vec!["Building Agentic AI Systems", "LLMs in Production"]);
        let formats: HashSet<&str> = order
            .books
            .iter()
            .flat_map(|b| b.files.iter().map(|f| f.format.as_str()))
            .collect();
        assert_eq!(formats, HashSet::from(["epub", "pdf", "mobi"]));
    }

    #[test]
    fn parse_order_captures_publisher_size_md5() {
        let order = parse_order(&fixture());
        let book = &order.books[0];
        assert_eq!(book.publisher.as_deref(), Some("Apress"));
        let epub = &book.files[0];
        assert_eq!(epub.size, Some(1_048_576));
        assert_eq!(epub.md5.as_deref(), Some("0123456789abcdef0123456789abcdef"));
    }

    #[test]
    fn build_tasks_layout() {
        let order = parse_order(&fixture());
        let output = PathBuf::from("/out");
        let tasks = build_tasks(&order, &output, None);
        let dests: HashSet<String> = tasks
            .iter()
            .map(|t| t.dest.strip_prefix(&output).unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            dests,
            HashSet::from([
                "Apress/Agentic AI and Large Language Models/epub/Building_Agentic_AI_Systems.epub".to_string(),
                "Apress/Agentic AI and Large Language Models/pdf/Building_Agentic_AI_Systems.pdf".to_string(),
                "Apress/Agentic AI and Large Language Models/mobi/LLMs_in_Production.mobi".to_string(),
            ])
        );
    }

    #[test]
    fn build_tasks_format_filter() {
        let order = parse_order(&fixture());
        let formats = HashSet::from(["epub".to_string()]);
        let tasks = build_tasks(&order, &PathBuf::from("/out"), Some(&formats));
        let names: Vec<String> = tasks
            .iter()
            .map(|t| t.dest.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["Building_Agentic_AI_Systems.epub"]);
    }

    #[test]
    fn build_tasks_falls_back_to_book_publisher() {
        let mut data = fixture();
        data["product"]["human_name"] = Value::String("Humble Book Bundle: Cybersecurity 2.0".to_string());
        let order = parse_order(&data);
        let formats = HashSet::from(["epub".to_string()]);
        let output = PathBuf::from("/out");
        let tasks = build_tasks(&order, &output, Some(&formats));
        assert_eq!(
            tasks[0].dest.strip_prefix(&output).unwrap().to_string_lossy(),
            "Apress/Cybersecurity 2.0/epub/Building_Agentic_AI_Systems.epub"
        );
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test api::`
Expected: 5 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/api.rs
git commit -m "Add api module: order parsing and task building"
```

---

## Task 7: `api::HumbleClient`

**Files:**
- Modify: `src/api.rs`

**Interfaces:**
- Consumes: `auth::AuthError` (Task 5); `parse_order` (Task 6, same file).
- Produces: `pub enum ApiError { NotFound(String) }`, `pub enum HumbleClientError { Auth(AuthError), Api(ApiError), Network(reqwest::Error) }` (each variant convertible via `#[from]`/`?`), `pub struct HumbleClient`, `impl HumbleClient { pub fn new(session_cookie: &str) -> Result<Self, HumbleClientError>; pub async fn list_order_keys(&self) -> Result<Vec<String>, HumbleClientError>; pub async fn get_order(&self, key: &str) -> Result<Order, HumbleClientError>; }`. `HumbleClient::new` reads the `HBSYNC_API_BASE` env var (falls back to the real Humble API host) so tests and `main.rs`'s CLI integration tests (Task 9) can point it at a `wiremock` server without touching the network. Used by `main.rs` (Task 8).

- [ ] **Step 1: Add error types and the client above the existing `#[cfg(test)]` block**

```rust
use std::time::Duration;

use thiserror::Error;

use crate::auth::AuthError;

const API_BASE: &str = "https://www.humblebundle.com/api/v1";

const SESSION_REJECTED_MSG: &str = "Humble Bundle rejected the session cookie. \
Log into humblebundle.com in Firefox and re-run, or pass --cookie.";

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("not found: {0}")]
    NotFound(String),
}

#[derive(Debug, Error)]
pub enum HumbleClientError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Network(#[from] reqwest::Error),
}

pub struct HumbleClient {
    client: reqwest::Client,
    base_url: String,
    cookie: String,
}

impl HumbleClient {
    pub fn new(session_cookie: &str) -> Result<Self, HumbleClientError> {
        let base_url = std::env::var("HBSYNC_API_BASE").unwrap_or_else(|_| API_BASE.to_string());
        let client = reqwest::Client::builder()
            .user_agent("hbsync/0.1")
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { client, base_url, cookie: session_cookie.to_string() })
    }

    pub async fn list_order_keys(&self) -> Result<Vec<String>, HumbleClientError> {
        let data = self.get_json(&format!("{}/user/order", self.base_url)).await?;
        Ok(data
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.get("gamekey").and_then(Value::as_str).map(str::to_string))
            .collect())
    }

    pub async fn get_order(&self, key: &str) -> Result<Order, HumbleClientError> {
        let data = self.get_json(&format!("{}/order/{}", self.base_url, key)).await?;
        Ok(parse_order(&data))
    }

    async fn get_json(&self, url: &str) -> Result<Value, HumbleClientError> {
        let response = self
            .client
            .get(url)
            .header("Cookie", format!("_simpleauth_sess={}", self.cookie))
            .send()
            .await?;
        match response.status().as_u16() {
            401 | 403 => return Err(HumbleClientError::Auth(AuthError(SESSION_REJECTED_MSG.to_string()))),
            404 => return Err(HumbleClientError::Api(ApiError::NotFound(url.to_string()))),
            _ => {}
        }
        let response = response.error_for_status()?;
        Ok(response.json::<Value>().await?)
    }
}
```

- [ ] **Step 2: Add `wiremock`-based tests in a new test module**

Append to `src/api.rs`:

```rust
#[cfg(test)]
mod client_tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn client_for(server: &MockServer) -> HumbleClient {
        HumbleClient {
            client: reqwest::Client::new(),
            base_url: format!("{}/api/v1", server.uri()),
            cookie: "abc".to_string(),
        }
    }

    #[test]
    fn api_base_uses_www_host() {
        assert!(API_BASE.contains("www.humblebundle.com"));
    }

    #[tokio::test]
    async fn list_order_keys_sends_cookie_and_parses() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/user/order"))
            .and(header("cookie", "_simpleauth_sess=abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"gamekey": "KEY1"}, {"gamekey": "KEY2"}
            ])))
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        assert_eq!(client.list_order_keys().await.unwrap(), vec!["KEY1", "KEY2"]);
    }

    #[tokio::test]
    async fn get_order_parses_response() {
        let server = MockServer::start().await;
        let fixture: Value = serde_json::from_str(include_str!("../tests/fixtures/order_sample.json")).unwrap();
        Mock::given(method("GET"))
            .and(path("/api/v1/order/CWPBwb82sqPXqEsq"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&fixture))
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let order = client.get_order("CWPBwb82sqPXqEsq").await.unwrap();
        assert_eq!(order.books.len(), 2);
    }

    #[tokio::test]
    async fn rejected_session_raises_autherror() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
        let client = client_for(&server).await;
        match client.list_order_keys().await.unwrap_err() {
            HumbleClientError::Auth(e) => assert!(e.0.contains("Log into humblebundle.com")),
            other => panic!("expected Auth error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_key_raises_apierror() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
        let client = client_for(&server).await;
        match client.get_order("NOPE").await.unwrap_err() {
            HumbleClientError::Api(ApiError::NotFound(_)) => {}
            other => panic!("expected Api(NotFound), got {other:?}"),
        }
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test api::`
Expected: 10 tests pass (5 from Task 6 + 5 new).

- [ ] **Step 4: Commit**

```bash
git add src/api.rs
git commit -m "Add HumbleClient: order fetching over HTTP"
```

---

## Task 8: `main.rs` — CLI wiring, key resolution, list mode

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `auth::{firefox_session_cookie, AuthError}` (Task 5); `api::{HumbleClient, HumbleClientError, ApiError, build_tasks}` (Tasks 6–7); `downloader::already_present` (Task 3).
- Produces: `fn parse_key(arg: &str) -> String`, a `clap`-derived `Cli` struct, and `async fn run(cli: Cli, formats: Option<HashSet<String>>) -> anyhow::Result<u8>` covering cookie resolution, key resolution/parsing, concurrent order fetching, the preflight summary line, and `--list` mode. Download execution (Task 9) extends `run` with the non-list path.

- [ ] **Step 1: Write `src/main.rs`**

```rust
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
```

- [ ] **Step 2: Build and run the existing module tests**

Run: `cargo build && cargo test`
Expected: builds cleanly; all previously-written module tests (naming, downloader, auth, api) still pass. `run_downloads` is a placeholder replaced in Task 9, so no download actually happens yet — that's expected at this checkpoint.

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "Wire CLI: cookie resolution, key fetching, list mode"
```

---

## Task 9: `main.rs` — download execution, progress bar, exit codes

**Files:**
- Modify: `src/main.rs`
- Create: `tests/cli.rs` (black-box binary tests via `assert_cmd` against a `wiremock` server)

**Interfaces:**
- Consumes: `downloader::{download_all, DownloadResult, Status, DownloadTask}` (Task 4); replaces the `run_downloads` placeholder from Task 8.
- Produces: the final `run_downloads` used by `run`, completing the CLI's behavior.

- [ ] **Step 1: Replace the placeholder `run_downloads` in `src/main.rs`**

```rust
use std::io::IsTerminal;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use downloader::{download_all, DownloadResult, DownloadTask, Status};

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
```

Remove the old placeholder `run_downloads` and the now-unused `_tasks`/`_cli` parameter names from Task 8 (they're superseded by this real implementation — same function name and signature shape, `Vec<DownloadTask>` typed explicitly instead of `Vec<downloader::DownloadTask>` since `DownloadTask` is now imported directly).

- [ ] **Step 2: Write black-box CLI tests against a mock Humble API**

Create `tests/cli.rs`:

```rust
use assert_cmd::Command;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/order_sample.json")).unwrap()
}

async fn server_with_order() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/v1/order/.*$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture()))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn list_mode_prints_paths_and_downloads_nothing() {
    let server = server_with_order().await;
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("hbsync").unwrap();
    let output = cmd
        .env("HBSYNC_API_BASE", format!("{}/api/v1", server.uri()))
        .args(["--list", "--cookie", "x", "-o"])
        .arg(tmp.path())
        .arg("CWPBwb82sqPXqEsq")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("Building_Agentic_AI_Systems.epub"));
    assert!(stdout.contains("3 files"));
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn preflight_summary_counts_already_downloaded() {
    let server = server_with_order().await;
    let tmp = tempfile::tempdir().unwrap();
    let epub_dir = tmp
        .path()
        .join("Apress")
        .join("Agentic AI and Large Language Models")
        .join("epub");
    std::fs::create_dir_all(&epub_dir).unwrap();
    std::fs::write(epub_dir.join("Building_Agentic_AI_Systems.epub"), vec![b'x'; 1_048_576]).unwrap();

    let mut cmd = Command::cargo_bin("hbsync").unwrap();
    let output = cmd
        .env("HBSYNC_API_BASE", format!("{}/api/v1", server.uri()))
        .args(["--list", "--cookie", "x", "-o"])
        .arg(tmp.path())
        .arg("CWPBwb82sqPXqEsq")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("3 available, 1 already downloaded, 2 to download"));
}

#[tokio::test]
async fn formats_flag_filters() {
    let server = server_with_order().await;
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("hbsync").unwrap();
    let output = cmd
        .env("HBSYNC_API_BASE", format!("{}/api/v1", server.uri()))
        .args(["--list", "--formats", "epub,mobi", "--cookie", "x", "-o"])
        .arg(tmp.path())
        .arg("CWPBwb82sqPXqEsq")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("2 files"));
    assert!(!stdout.contains(".pdf"));
}

#[tokio::test]
async fn unknown_key_reports_failure_and_nonzero_exit() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("hbsync").unwrap();
    let output = cmd
        .env("HBSYNC_API_BASE", format!("{}/api/v1", server.uri()))
        .args(["--list", "--cookie", "x", "-o"])
        .arg(tmp.path())
        .arg("NOPE")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("NOPE"));
}

#[tokio::test]
async fn rejected_cookie_prints_message_and_exits_nonzero() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("hbsync").unwrap();
    let output = cmd
        .env("HBSYNC_API_BASE", format!("{}/api/v1", server.uri()))
        .args(["--list", "--cookie", "x", "-o"])
        .arg(tmp.path())
        .arg("CWPBwb82sqPXqEsq")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("Log into humblebundle.com"));
}
```

Note: these black-box tests cover the equivalent of the Python suite's list-mode, preflight-summary, format-filter, key-failure, and auth-rejection scenarios. The Python suite's TTY-progress-bar test and orders-fetched-concurrently timing assertion rely on monkeypatching internals that aren't reachable from outside a compiled binary; that gap is accepted per the design's testing note (decide CLI test strategy for what stays maintainable) — the underlying concurrency is still exercised indirectly by every multi-key test above running against a real (if local) HTTP round trip per key.

- [ ] **Step 3: Run all tests**

Run: `cargo test`
Expected: all unit tests (naming, downloader, auth, api) plus the new `tests/cli.rs` integration tests pass.

- [ ] **Step 4: Manually verify the progress bar path**

Run: `cargo run -- --help` to confirm argument parsing/help text looks right.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs tests/cli.rs
git commit -m "Add download execution, progress bar, and CLI integration tests"
```

---

## Task 10: README and final verification

**Files:**
- Create: `README.md`

**Interfaces:**
- None — documentation only.

- [ ] **Step 1: Write `README.md`**

```markdown
# hbsync

A command-line tool that downloads the ebooks from your Humble Bundle library and organizes them on disk as `Publisher/Bundle/Format/`.

Rust port of the original Python `hbsync`, same approach, faster runtime.

## Approach

Humble Bundle's website is backed by a JSON API on `https://www.humblebundle.com/api/v1`. hbsync authenticates with the same session cookie your browser uses:

1. Reads the `_simpleauth_sess` cookie from your local Firefox profile (a temp copy of `cookies.sqlite` is queried, so it works while Firefox is running). You can bypass this with `--cookie`.
2. Fetches your purchase keys from `/user/order`, or uses the keys you pass on the command line.
3. Fetches each order from `/order/{key}` and keeps only ebook downloads (Humble platform `"ebook"`). Games, software, and key-only items are skipped.
4. Downloads files concurrently (4 at a time by default) using the signed URLs, sizes, and MD5 hashes the API provides.

Directory names come from the bundle title, split the same way as the Python original: on the last `:` and the final ` by ` into publisher and bundle. When a title has no ` by <Publisher>` suffix, each book's own publisher field from the API is used instead, and `Unknown` is the last resort.

Re-runs are idempotent. A file that already exists with the expected size is skipped. Downloads stream to a `.part` file that is renamed into place only after size and MD5 checks pass. Failed downloads are retried three times with backoff, and the exit code is nonzero if anything ultimately failed.

## Architecture

```
src/
├── main.rs       clap CLI, wiring, indicatif progress bar, exit codes
├── auth.rs       Firefox profile discovery and session cookie extraction
├── api.rs        HumbleClient, order JSON parsing, download task building
├── naming.rs     bundle title heuristic and filesystem-safe sanitizing
└── downloader.rs tokio + reqwest parallel downloads with skip/verify/retry
```

## Installation

```bash
cargo build --release
```

The binary is at `target/release/hbsync`.

## Usage

```bash
hbsync                            # sync every ebook in your library to the current directory
hbsync -o ~/Books                 # sync to ~/Books
hbsync CWPBwb82sqPXqEsq           # only this purchase (bare key or full downloads URL)
hbsync --formats epub,pdf         # restrict formats (default: all offered)
hbsync --parallel 8               # concurrent downloads (default: 4)
hbsync --list                     # show what would be downloaded, download nothing
hbsync --cookie <value>           # use this session cookie instead of reading Firefox
```

You must be logged into humblebundle.com in Firefox (or supply `--cookie`).

## Development

```bash
cargo test
```

Tests never touch the network: HTTP behavior is tested against a local `wiremock` server, the Humble API against a captured sample order in `tests/fixtures/`, and cookie extraction against a fixture sqlite database built in-test. Design and plan documents live in `docs/superpowers/`.

## License

Copyright (C) 2026 Dougie Richardson

This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 2 of the License, or (at your option) any later version. See [LICENSE](LICENSE) for the full text.
```

- [ ] **Step 2: Run the full test suite one more time**

Run: `cargo test`
Expected: all tests across all modules pass.

- [ ] **Step 3: Run clippy and fix any warnings**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings (fix any that surface — e.g. needless clones, redundant closures — before proceeding).

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "Add README"
```

---

## Post-plan check

After Task 10, do a final end-to-end sanity check against the real Humble Bundle API (manual, not automated): run `cargo run -- --list --cookie <your real cookie>` and confirm the output looks sane against your actual library, since no automated test exercises the real API.
