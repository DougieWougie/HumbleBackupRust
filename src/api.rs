//! Humble Bundle JSON API client and order-to-download-task mapping.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::auth::AuthError;
use crate::downloader::DownloadTask;
use crate::naming::{parse_bundle_title, sanitize};

const API_BASE: &str = "https://www.humblebundle.com/api/v1";

/// Total deadline for an API request. Order metadata is small, so a whole-
/// request timeout is appropriate here (unlike file downloads, which get an
/// idle-gap timeout instead — see `main::http_client`).
const API_TIMEOUT: Duration = Duration::from_secs(30);

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

/// Resolve the API base URL, honoring `HBSYNC_API_BASE`.
///
/// Every request carries the Humble session cookie, so an unvalidated
/// override would let anything able to set an environment variable redirect
/// that cookie to a host of its choosing. The override therefore only
/// accepts loopback addresses, which is all the integration tests need.
fn resolve_base_url() -> String {
    let Ok(override_url) = std::env::var("HBSYNC_API_BASE") else {
        return API_BASE.to_string();
    };
    if is_loopback_url(&override_url) {
        override_url
    } else {
        eprintln!(
            "\u{26a0} ignoring HBSYNC_API_BASE={override_url}: only loopback addresses are \
             accepted, because the session cookie is sent to this host."
        );
        API_BASE.to_string()
    }
}

pub fn is_loopback_url(candidate: &str) -> bool {
    url::Url::parse(candidate).is_ok_and(|url| match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(host)) => host == "localhost",
        None => false,
    })
}

impl HumbleClient {
    /// Takes the process-wide [`reqwest::Client`] so the connection pool is
    /// shared with the downloader.
    pub fn new(client: reqwest::Client, session_cookie: &str) -> Self {
        Self { client, base_url: resolve_base_url(), cookie: session_cookie.to_string() }
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
            .timeout(API_TIMEOUT)
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadFile {
    pub format: String,
    pub url: String,
    pub size: Option<u64>,
    pub md5: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Book {
    pub title: String,
    pub publisher: Option<String>,
    pub files: Vec<DownloadFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    use std::path::PathBuf;

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

    #[test]
    fn only_loopback_overrides_are_accepted() {
        // The session cookie goes to whatever HBSYNC_API_BASE names, so a
        // non-loopback override must never be honored.
        assert!(is_loopback_url("http://127.0.0.1:8080/api/v1"));
        assert!(is_loopback_url("http://localhost:8080/api/v1"));
        assert!(is_loopback_url("http://[::1]:8080/api/v1"));
        assert!(!is_loopback_url("https://evil.example.com/api/v1"));
        assert!(!is_loopback_url("https://127.0.0.1.evil.example.com/api/v1"));
        assert!(!is_loopback_url("not a url"));
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
