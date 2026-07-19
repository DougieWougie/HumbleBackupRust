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
