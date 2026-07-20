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
