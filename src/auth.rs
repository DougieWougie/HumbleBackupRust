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
