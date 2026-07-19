# hbsync Rust port — design

## Summary

Port the Python `hbsync` CLI (in the sibling `bulk` project) to Rust, in this
repository (`bulk_rust`). Same product, same architecture and behavior, faster
runtime. Behavior may diverge in minor, cosmetic ways (e.g. progress bar
rendering via a library instead of hand-rolled) where a Rust idiom fits
better, but CLI flags, output semantics, and exit-code logic stay the same.

## Source of truth

The existing Python implementation at `/home/dougiewougie/Projects/bulk/hbsync/`
is the behavioral reference:

- `auth.py` — Firefox session cookie extraction
- `naming.py` — bundle title parsing / path sanitizing
- `api.py` — Humble Bundle JSON API client + order→task mapping
- `downloader.py` — async parallel download with skip/verify/retry
- `cli.py` — argument parsing, wiring, progress output, exit codes

## Architecture

One Rust module per Python module, same responsibilities and one-directional
data flow: `auth` produces a cookie → `api` turns API responses into
`DownloadTask`s (via `naming` for path segments) → `downloader` executes them
under a concurrency limit → `main` reports results and sets the exit code.

```
src/
├── main.rs       clap CLI, wiring, indicatif progress bar, exit codes
├── auth.rs       Firefox profile discovery + cookie extraction (rusqlite)
├── naming.rs     bundle title parsing + path sanitizing (pure functions)
├── api.rs        HumbleClient, Order/Book/DownloadFile types, order→task mapping
└── downloader.rs tokio + reqwest parallel downloads with skip/verify/retry
```

## Crate choices

- `tokio` (rt-multi-thread) — async runtime
- `reqwest` (json, stream, cookies features) — HTTP client
- `clap` (derive) — argument parsing
- `indicatif` — progress bar
- `rusqlite` (bundled feature, no system libsqlite3 dependency) — Firefox
  cookie DB reads
- `serde` / `serde_json` — JSON parsing
- `md-5` — MD5 verification
- `thiserror` — typed errors per module (`AuthError`, `ApiError`,
  `IntegrityError`)
- `anyhow` — error collapsing at the CLI boundary
- `tempfile` — scratch copy of the cookie DB before reading
- Dev-only: `wiremock` (HTTP mocking), `assert_cmd` + `predicates` (CLI
  integration tests, if warranted — see Testing)

## Module behavior

### `auth`

`firefox_session_cookie(firefox_dir: Option<&Path>) -> Result<String, AuthError>`:
glob `<firefox_dir>/*/cookies.sqlite` (default `~/.mozilla/firefox`), sort
by mtime descending, and for each DB copy it into a tempdir (Firefox holds
the original locked while running) and query:

```sql
SELECT value FROM moz_cookies
WHERE host LIKE '%humblebundle.com' AND name = '_simpleauth_sess'
ORDER BY expiry DESC LIMIT 1
```

Return the first non-empty match across all profiles, in mtime order. If none
found, `AuthError` with the same guidance message as Python ("log into
humblebundle.com in Firefox and re-run, or pass --cookie").

### `naming`

- `sanitize(name: &str) -> String` — replace NUL and `/` with `_`, collapse
  whitespace runs to a single space, trim, strip trailing `.`/space, fall
  back to `"Unnamed"` if empty.
- `parse_bundle_title(title: &str) -> (Option<String>, String)` — split on
  the last `:`, then on the last `" by "` in the tail; return
  `(Some(publisher), bundle)` when both halves are non-empty after
  trimming, otherwise `(None, tail_or_title)`.

Pure functions, no I/O — ported 1:1 from `test_naming.py` cases.

### `api`

Types: `DownloadFile { format, url, size: Option<u64>, md5: Option<String> }`,
`Book { title, publisher: Option<String>, files: Vec<DownloadFile> }`,
`Order { key, title, books: Vec<Book> }`.

`parse_order(data: &serde_json::Value) -> Order` — walk `subproducts[].downloads[]`
filtered to `platform == "ebook"`, then `download_struct[]`, keeping entries
with both `url.web` and `name`; format is the lowercased name with a leading
`.` stripped. Skip books with no files. `serde_json::Value` walking (not a
rigid `Deserialize` struct) matches the Python `.get()`-chain tolerance for
missing/loosely-typed fields.

`build_tasks(order: &Order, output: &Path, formats: Option<&HashSet<String>>) -> Vec<DownloadTask>`
— derive `(publisher, bundle_dir)` from `naming::parse_bundle_title(order.title)`;
per book, publisher falls back to the book's own publisher then `"Unknown"`;
per file, skip if `formats` is set and doesn't contain the file's format;
filename is the URL-decoded last path segment, or
`{sanitize(book.title)}.{format}` if empty; destination is
`output/sanitize(publisher)/sanitize(bundle_dir)/sanitize(format)/sanitize(filename)`.

`HumbleClient` — wraps a `reqwest::Client` with the `_simpleauth_sess` cookie
set and `User-Agent: hbsync/0.1`, 30s timeout, redirects followed.
`list_order_keys()` hits `/user/order`; `get_order(key)` hits `/order/{key}`
and parses via `parse_order`. 401/403 → `AuthError`, 404 → `ApiError`, other
non-2xx → generic `ApiError`.

### `downloader`

`DownloadTask { url, dest: PathBuf, size: Option<u64>, md5: Option<String> }`,
`Result { task, status: Downloaded | Skipped | Failed(String) }`.

`already_present(task) -> bool` — dest exists and (no expected size, or size
matches).

`download_all(tasks, parallel, client, backoff, on_result) -> Vec<Result>` —
run all tasks concurrently under a semaphore of size `parallel`; each task:
skip if already present, else stream to `dest.part`, updating an MD5 digest
per chunk; on success verify size and MD5 (mismatches are an `IntegrityError`
that triggers a retry like any other transient error); on final success
rename `.part` → `dest`; on any error delete the `.part` file. Retry up to
`backoff.len()` additional times with the given sleep durations between
attempts (default `[1s, 2s, 4s]`); last error message reported on ultimate
failure. `on_result` callback fires per completed task (used for progress
reporting).

### `main`

clap-derived args: positional `keys` (purchase keys or `...?key=...` URLs,
default: whole library), `-o/--output` (default `.`), `--formats`
(comma-separated, default: all), `--parallel` (default 4), `--list`
(dry run), `--cookie` (skip Firefox read).

Flow: resolve cookie (flag or `auth::firefox_session_cookie`) → build
`HumbleClient` → resolve keys (explicit args, extracting the `key=` query
param when a URL is passed, or `list_order_keys()`) → fetch orders
concurrently under the same `--parallel` limit, printing `✗ {key}: {err}` to
stderr and counting key failures without aborting the run → build tasks from
successful orders → print
`"{available} available, {already_downloaded} already downloaded, {to_download} to download"`.

If `--list`: print each task's destination path, then `"{n} files"`, exit
1 if any key failed else 0.

Otherwise: download with an indicatif progress bar (only when stdout is a
TTY) tracking completed/total/failed; after completion print one line per
failed download (`✗ {dest} ({error})`), then the summary line
`"{downloaded} downloaded, {skipped} skipped, {failed} failed"`. Exit 1 if
any download or key fetch failed, else 0.

Top-level error handling: `AuthError` and network errors print their message
to stderr and exit 1, matching Python's `try/except` in `main()`.

## Testing

- `naming` — pure unit tests, direct translation of `test_naming.py` cases.
- `api` — `wiremock` local server + a captured order JSON fixture (ported
  from `tests/fixtures/`), asserting parsed `Order`/`DownloadTask` output
  against the same expectations as `test_api.py`.
- `downloader` — `wiremock` serving bytes with controllable size/MD5/failure
  injection, asserting skip/retry/verify/`.part`-cleanup behavior, ported
  from `test_downloader.py`.
- `auth` — build a fixture sqlite DB with the `moz_cookies` schema (via
  `rusqlite` in test setup, replacing the Python fixture DB) and assert
  extraction, including the "most recently modified profile wins" and
  "no cookie found" cases.
- `cli` — argument-parsing and exit-code logic tests; use `assert_cmd` for
  true end-to-end binary tests if that proves clean, otherwise test the
  argument-to-behavior mapping in-process — decided during implementation
  based on what stays maintainable.

Tests never touch the real network, matching the Python suite's guarantee.

## Out of scope

- Feature parity beyond what's listed above (no new flags or behaviors).
- Byte-identical stdout formatting for the progress bar (indicatif's
  rendering differs cosmetically from the hand-rolled bar; everything else
  textual stays matched).
- Packaging/distribution (crates.io publish, binary releases) — not
  requested.
