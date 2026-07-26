# hbsync

A command-line tool that downloads the ebooks from your Humble Bundle library and organizes them on disk as `Publisher/Bundle/Format/`.

Rust port of the original Python `hbsync`, same approach, faster runtime.

## Approach

Humble Bundle's website is backed by a JSON API on `https://www.humblebundle.com/api/v1`. hbsync authenticates with the same session cookie your browser uses:

1. Reads the `_simpleauth_sess` cookie from your local Firefox profile (a temp copy of `cookies.sqlite` is queried, so it works while Firefox is running). You can bypass this with `--cookie-stdin`, the `HBSYNC_COOKIE` environment variable, or `--cookie`.
2. Fetches your purchase keys from `/user/order`, or uses the keys you pass on the command line.
3. Fetches each order from `/order/{key}` and keeps only ebook downloads (Humble platform `"ebook"`). Games, software, and key-only items are skipped.
4. Downloads files concurrently (4 at a time by default) using the signed URLs, sizes, and MD5 hashes the API provides.

Directory names come from the bundle title, split the same way as the Python original: on the last `:` and the final ` by ` into publisher and bundle. When a title has no ` by <Publisher>` suffix, each book's own publisher field from the API is used instead, and `Unknown` is the last resort.

Re-runs are idempotent. A file that already exists with the expected size is skipped. Downloads stream to a `.part` file that is renamed into place only after size and MD5 checks pass. A `.part` left behind by an interrupted run is resumed with an HTTP `Range` request rather than refetched, with the bytes already on disk folded into the MD5. Failures are retried three times with backoff when retrying could plausibly help (connection errors, timeouts, 5xx, 429, truncated transfers) and reported immediately when it couldn't (404, a checksum that simply disagrees). The exit code is nonzero if anything ultimately failed.

Order metadata (titles, sizes, MD5s, formats — everything except the signed download URL, which expires) is cached under `~/.cache/hbsync/orders/` after the first fetch, since a bundle's contents never change once purchased. On a re-run, if every file an order would produce is already on disk, that order is resolved entirely from the cache with no network request; only orders with something still missing are refetched (which also refreshes the cache entry). This makes repeat syncs of a mostly-downloaded library fast even with a large number of bundles. Pass `--refresh` to bypass the cache and refetch everything.

## Architecture

```
src/
├── main.rs       clap CLI, wiring, indicatif progress bar, exit codes
├── auth.rs       Firefox profile discovery and session cookie extraction
├── api.rs        HumbleClient, order JSON parsing, download task building
├── cache.rs      on-disk cache of parsed order metadata, keyed by gamekey
├── naming.rs     bundle title heuristic and filesystem-safe sanitizing
└── downloader.rs tokio + reqwest parallel downloads with skip/verify/retry
```

## Installation

```bash
cargo build --release
```

The binary is at `target/release/hbsync`. TLS is rustls, so there is no system OpenSSL dependency, and the release profile enables thin LTO.

## Usage

```bash
hbsync                            # sync every ebook in your library to the current directory
hbsync -o ~/Books                 # sync to ~/Books
hbsync CWPBwb82sqPXqEsq           # only this purchase (bare key or full downloads URL)
hbsync --formats epub,pdf         # restrict formats (default: all offered)
hbsync --parallel 8               # concurrent downloads (default: 4)
hbsync --list                     # show what would be downloaded, download nothing
hbsync --cookie-stdin             # read the session cookie from stdin
hbsync --cookie <value>           # use this session cookie instead of reading Firefox
hbsync --refresh                  # bypass the order metadata cache and refetch everything
```

You must be logged into humblebundle.com in Firefox, or supply the cookie yourself.

## Handling of the session cookie

The `_simpleauth_sess` cookie is full access to your Humble Bundle account, so hbsync tries not to leak it:

- Cookie sources, in order of precedence: `--cookie-stdin`, `--cookie`, `$HBSYNC_COOKIE`, the Firefox profile. `--cookie` puts the value in the process list where any other local user can read it — `pass show humble | hbsync --cookie-stdin` avoids that.
- The cookie is only ever sent to `https://www.humblebundle.com`. `HBSYNC_API_BASE` can redirect the API client, but only to a loopback address (this exists for the integration tests); any other value is ignored with a warning.
- Download URLs from the order API are fetched over https only. A plaintext URL fails the task instead of being downloaded, except on loopback.
- Cached order metadata contains the signed download URLs for your library, so `~/.cache/hbsync/` is created mode `0700` with entries mode `0600`.

## Development

```bash
cargo test
```

Tests never touch the network: HTTP behavior is tested against a local `wiremock` server, the Humble API against a captured sample order in `tests/fixtures/`, and cookie extraction against a fixture sqlite database built in-test. Design and plan documents live in `docs/superpowers/`.

## License

Copyright (C) 2026 Dougie Richardson

This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 2 of the License, or (at your option) any later version. See [LICENSE](LICENSE) for the full text.
