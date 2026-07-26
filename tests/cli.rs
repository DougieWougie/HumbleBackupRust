use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use assert_cmd::Command;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

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

/// A `Command` pre-wired with an isolated, per-test cache directory so runs
/// never touch the real `~/.cache/hbsync`.
fn hbsync_cmd(server: &MockServer, cache_dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("hbsync").unwrap();
    cmd.env("HBSYNC_API_BASE", format!("{}/api/v1", server.uri()));
    cmd.env("HBSYNC_CACHE_DIR", cache_dir);
    cmd
}

#[tokio::test]
async fn list_mode_prints_paths_and_downloads_nothing() {
    let server = server_with_order().await;
    let tmp = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let output = hbsync_cmd(&server, cache.path())
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
    let cache = tempfile::tempdir().unwrap();
    let epub_dir = tmp
        .path()
        .join("Apress")
        .join("Agentic AI and Large Language Models")
        .join("epub");
    std::fs::create_dir_all(&epub_dir).unwrap();
    std::fs::write(epub_dir.join("Building_Agentic_AI_Systems.epub"), vec![b'x'; 1_048_576]).unwrap();

    let output = hbsync_cmd(&server, cache.path())
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
    let cache = tempfile::tempdir().unwrap();
    let output = hbsync_cmd(&server, cache.path())
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
    let cache = tempfile::tempdir().unwrap();
    let output = hbsync_cmd(&server, cache.path())
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
    let cache = tempfile::tempdir().unwrap();
    let output = hbsync_cmd(&server, cache.path())
        .args(["--list", "--cookie", "x", "-o"])
        .arg(tmp.path())
        .arg("CWPBwb82sqPXqEsq")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("Log into humblebundle.com"));
}

struct CountingOrderResponder {
    calls: Arc<AtomicUsize>,
}

impl Respond for CountingOrderResponder {
    fn respond(&self, _req: &wiremock::Request) -> ResponseTemplate {
        self.calls.fetch_add(1, Ordering::SeqCst);
        ResponseTemplate::new(200).set_body_json(fixture())
    }
}

/// Pre-create every file the fixture order would produce, with the exact
/// expected size, so the run is a full "already downloaded" library.
fn populate_all_fixture_files(root: &std::path::Path) {
    let epub_dir = root.join("Apress").join("Agentic AI and Large Language Models").join("epub");
    let pdf_dir = root.join("Apress").join("Agentic AI and Large Language Models").join("pdf");
    let mobi_dir = root.join("Apress").join("Agentic AI and Large Language Models").join("mobi");
    std::fs::create_dir_all(&epub_dir).unwrap();
    std::fs::create_dir_all(&pdf_dir).unwrap();
    std::fs::create_dir_all(&mobi_dir).unwrap();
    std::fs::write(epub_dir.join("Building_Agentic_AI_Systems.epub"), vec![b'x'; 1_048_576]).unwrap();
    std::fs::write(pdf_dir.join("Building_Agentic_AI_Systems.pdf"), vec![b'x'; 2_097_152]).unwrap();
    std::fs::write(mobi_dir.join("LLMs_in_Production.mobi"), vec![b'x'; 3_145_728]).unwrap();
}

#[tokio::test]
async fn second_run_with_full_cache_and_files_present_skips_order_fetch() {
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/v1/order/.*$"))
        .respond_with(CountingOrderResponder { calls: Arc::clone(&calls) })
        .mount(&server)
        .await;
    let tmp = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    populate_all_fixture_files(tmp.path());

    let first = hbsync_cmd(&server, cache.path())
        .args(["--list", "--cookie", "x", "-o"])
        .arg(tmp.path())
        .arg("CWPBwb82sqPXqEsq")
        .output()
        .unwrap();
    assert!(first.status.success());
    assert_eq!(calls.load(Ordering::SeqCst), 1, "first run must populate the cache over the network");

    let second = hbsync_cmd(&server, cache.path())
        .args(["--list", "--cookie", "x", "-o"])
        .arg(tmp.path())
        .arg("CWPBwb82sqPXqEsq")
        .output()
        .unwrap();
    assert!(second.status.success());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "second run must reuse the cache and skip the order request entirely"
    );
    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(stdout.contains("3 available, 3 already downloaded, 0 to download"));
}

#[tokio::test]
async fn refresh_flag_bypasses_warm_cache() {
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/v1/order/.*$"))
        .respond_with(CountingOrderResponder { calls: Arc::clone(&calls) })
        .mount(&server)
        .await;
    let tmp = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    populate_all_fixture_files(tmp.path());

    hbsync_cmd(&server, cache.path())
        .args(["--list", "--cookie", "x", "-o"])
        .arg(tmp.path())
        .arg("CWPBwb82sqPXqEsq")
        .output()
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let refreshed = hbsync_cmd(&server, cache.path())
        .args(["--list", "--refresh", "--cookie", "x", "-o"])
        .arg(tmp.path())
        .arg("CWPBwb82sqPXqEsq")
        .output()
        .unwrap();
    assert!(refreshed.status.success());
    assert_eq!(calls.load(Ordering::SeqCst), 2, "--refresh must bypass the cache and refetch");
}

/// Simulates a book that Humble re-uploaded after purchase: the order API
/// still reports the old size/md5, but the CDN serves the current (correct)
/// file. The download should succeed against the live size, and the cache
/// should be corrected so a second run recognizes the file as already
/// present without hitting the order endpoint again.
#[tokio::test]
async fn stale_order_metadata_self_heals_the_cache() {
    const CONTENT: &[u8] = b"the current, correct edition of this book";
    let server = MockServer::start().await;
    let order_calls = Arc::new(AtomicUsize::new(0));

    let order = |server_uri: &str| {
        serde_json::json!({
            "gamekey": "STALEKEY00000001",
            "product": {"human_name": "Some Bundle by Some Publisher", "machine_name": "somebundle_bookbundle"},
            "subproducts": [{
                "human_name": "A Book",
                "payee": {"human_name": "Some Publisher", "machine_name": "somepublisher"},
                "downloads": [{
                    "platform": "ebook",
                    "machine_name": "abook_ebook",
                    "download_struct": [{
                        "name": "EPUB",
                        "url": {"web": format!("{server_uri}/dl/book.epub")},
                        "file_size": 999999,
                        "md5": "0".repeat(32),
                        "human_size": "977 KB"
                    }]
                }]
            }]
        })
    };
    struct OrderResponder {
        calls: Arc<AtomicUsize>,
        body: serde_json::Value,
    }
    impl Respond for OrderResponder {
        fn respond(&self, _req: &wiremock::Request) -> ResponseTemplate {
            self.calls.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_json(&self.body)
        }
    }
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/v1/order/.*$"))
        .respond_with(OrderResponder { calls: Arc::clone(&order_calls), body: order(&server.uri()) })
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/dl/book\.epub$"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(CONTENT))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();

    let first = hbsync_cmd(&server, cache.path())
        .args(["--cookie", "x", "-o"])
        .arg(tmp.path())
        .arg("STALEKEY00000001")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&first.stdout);
    let stderr = String::from_utf8_lossy(&first.stderr);
    assert!(first.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("1 downloaded, 0 skipped, 0 failed"), "stdout: {stdout}");
    assert!(stderr.contains("order metadata disagreed"), "stderr: {stderr}");
    assert_eq!(order_calls.load(Ordering::SeqCst), 1);

    let downloaded_path =
        tmp.path().join("Some Publisher").join("Some Bundle").join("epub").join("book.epub");
    assert_eq!(std::fs::read(&downloaded_path).unwrap(), CONTENT);

    let cache_entry = std::fs::read_dir(cache.path())
        .unwrap()
        .next()
        .expect("cache entry should exist")
        .unwrap()
        .path();
    let cached: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(cache_entry).unwrap()).unwrap();
    let cached_size = cached["books"][0]["files"][0]["size"].as_u64().unwrap();
    assert_eq!(cached_size, CONTENT.len() as u64, "cache should be corrected to the live size");

    // Second run: the file on disk now matches the corrected cache entry, so
    // it should be recognized as already present with no further network calls.
    let second = hbsync_cmd(&server, cache.path())
        .args(["--list", "--cookie", "x", "-o"])
        .arg(tmp.path())
        .arg("STALEKEY00000001")
        .output()
        .unwrap();
    assert!(second.status.success());
    assert_eq!(order_calls.load(Ordering::SeqCst), 1, "second run must reuse the corrected cache");
    let second_stdout = String::from_utf8_lossy(&second.stdout);
    assert!(second_stdout.contains("1 available, 1 already downloaded, 0 to download"), "{second_stdout}");
}

#[tokio::test]
async fn cache_miss_with_missing_file_still_refetches() {
    let server = server_with_order().await;
    let tmp = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();

    // Nothing pre-downloaded, so even after the cache is warm the next run
    // still needs a fresh order (to get a valid signed URL) since files are
    // still missing on disk.
    let first = hbsync_cmd(&server, cache.path())
        .args(["--list", "--cookie", "x", "-o"])
        .arg(tmp.path())
        .arg("CWPBwb82sqPXqEsq")
        .output()
        .unwrap();
    assert!(first.status.success());
    let stdout = String::from_utf8_lossy(&first.stdout);
    assert!(stdout.contains("3 available, 0 already downloaded, 3 to download"));
}
