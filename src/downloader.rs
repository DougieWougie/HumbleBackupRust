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
        Ok(meta) => task.size.map_or(true, |size| meta.len() == size),
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
