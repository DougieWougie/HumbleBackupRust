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
