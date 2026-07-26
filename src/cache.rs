//! On-disk cache of parsed order metadata, keyed by gamekey.
//!
//! Order composition (titles, sizes, MD5s, formats) never changes once a
//! bundle is purchased, so caching it lets a re-run that finds every file
//! already on disk skip the order-detail network request entirely. The
//! signed download URL isn't trustworthy long-term (it carries a `ttl`
//! parameter), so a cache hit only short-circuits the network call when
//! every resulting task is already present; otherwise the caller re-fetches
//! to get a fresh URL and refreshes the cache entry.

use std::path::{Path, PathBuf};

use crate::api::Order;
use crate::naming::sanitize;

/// Resolve the cache directory, honoring `HBSYNC_CACHE_DIR` (used by tests
/// to isolate runs) and `XDG_CACHE_HOME`, falling back to `~/.cache`.
/// Returns `None` if no location can be determined (e.g. `$HOME` unset),
/// in which case callers should simply skip caching.
pub fn cache_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("HBSYNC_CACHE_DIR") {
        return Some(PathBuf::from(dir));
    }
    let base = match std::env::var_os("XDG_CACHE_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".cache"),
    };
    Some(base.join("hbsync").join("orders"))
}

fn entry_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{}.json", sanitize(key)))
}

pub fn load_order(dir: &Path, key: &str) -> Option<Order> {
    let data = std::fs::read_to_string(entry_path(dir, key)).ok()?;
    serde_json::from_str(&data).ok()
}

/// Cached orders hold the signed, `ttl`-bearing download URLs for the whole
/// library, so they are readable only by their owner.
#[cfg(unix)]
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // `mode` only applies when creating, so tighten entries that an earlier
    // version of hbsync may have left world-readable.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(data)
}

#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, data)
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

pub fn save_order(dir: &Path, key: &str, order: &Order) {
    if create_private_dir(dir).is_err() {
        return;
    }
    if let Ok(data) = serde_json::to_string(order) {
        let _ = write_private(&entry_path(dir, key), data.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Book, DownloadFile};
    use tempfile::tempdir;

    fn sample_order() -> Order {
        Order {
            key: "ABC123".to_string(),
            title: "Some Bundle by Some Publisher".to_string(),
            books: vec![Book {
                title: "A Book".to_string(),
                publisher: Some("Some Publisher".to_string()),
                files: vec![DownloadFile {
                    format: "epub".to_string(),
                    url: "https://dl.test/a.epub".to_string(),
                    size: Some(123),
                    md5: Some("abc".to_string()),
                }],
            }],
        }
    }

    #[test]
    fn round_trips_through_save_and_load() {
        let dir = tempdir().unwrap();
        let order = sample_order();
        save_order(dir.path(), &order.key, &order);
        let loaded = load_order(dir.path(), &order.key).unwrap();
        assert_eq!(loaded.key, order.key);
        assert_eq!(loaded.title, order.title);
        assert_eq!(loaded.books[0].files[0].md5, order.books[0].files[0].md5);
    }

    /// Cache entries carry signed download URLs, so they must not be
    /// readable by other users on the machine.
    #[cfg(unix)]
    #[test]
    fn entries_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let nested = dir.path().join("hbsync").join("orders");
        let order = sample_order();
        save_order(&nested, &order.key, &order);

        let entry = entry_path(&nested, &order.key);
        assert_eq!(std::fs::metadata(&entry).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&nested).unwrap().permissions().mode() & 0o777, 0o700);
    }

    /// A cache written by an older version could be world-readable; rewriting
    /// it must tighten the mode rather than preserve it.
    #[cfg(unix)]
    #[test]
    fn rewriting_tightens_a_world_readable_entry() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let order = sample_order();
        let entry = entry_path(dir.path(), &order.key);
        std::fs::write(&entry, "{}").unwrap();
        std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o644)).unwrap();

        save_order(dir.path(), &order.key, &order);
        assert_eq!(std::fs::metadata(&entry).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn missing_entry_is_none() {
        let dir = tempdir().unwrap();
        assert!(load_order(dir.path(), "NOPE").is_none());
    }

    #[test]
    fn key_is_sanitized_for_the_filename() {
        let dir = tempdir().unwrap();
        let mut order = sample_order();
        order.key = "weird/../key".to_string();
        save_order(dir.path(), &order.key, &order);
        // Sanitizing must prevent escaping the cache directory.
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 1);
        assert!(load_order(dir.path(), &order.key).is_some());
    }
}
