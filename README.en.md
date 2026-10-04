<div align="center">

<img src="assets/logo.svg" alt="HeiGate" width="120" />

# HeiGate

**An AI API gateway that runs on your own computer (macOS or Windows). Multi-channel routing, automatic failover, usage tracking — your data never leaves the machine.**

[![CI](https://github.com/Heiheiaoo/HeiGate-Desktop/actions/workflows/ci.yml/badge.svg)](https://github.com/Heiheiaoo/HeiGate-Desktop/actions/workflows/ci.yml)
[![Vue](https://img.shields.io/badge/Vue-3-4FC08D.svg)](https://vuejs.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

[中文](README.md) | English

</div>

## What it does

If you juggle several AI API endpoints — official keys, a relay, a local Ollama — HeiGate puts them behind one local address. Your coding tools point at the desktop default `http://127.0.0.1:18080` and it handles the rest:

- **Multi-channel routing** — pick a channel by latency, a quality score, or round robin
- **Automatic failover** — when a channel errors, rate-limits, or times out, the request moves to the next one; repeated failures trip a circuit breaker with a cooldown
- **Protocol translation** — talk to an OpenAI-compatible upstream with an Anthropic client; requests and responses (including streaming) are converted on the fly
- **Health probes** — on demand, exercise the primary model with a tiny arithmetic question, with queued checks for every configured model when you ask for them; nothing spends quota in the background
- **Usage tracking** — every request records its channel, model, tokens, time to first token, and failover chain locally, with daily/model/channel rollups
- **Client access** — connect Claude Code, Cursor, Codex CLI, and other clients with the gateway URL and API key

Everything lives in a local SQLite file (macOS: `~/Library/Application Support/HeiGate`; Windows: `%APPDATA%\HeiGate`). Nothing is uploaded. The gateway listens on loopback only.

## Install

Download the matching build from [Releases](https://github.com/Heiheiaoo/HeiGate-Desktop/releases):

- Apple silicon Mac (arm64): quit HeiGate, open the `.dmg`, and drag it onto the `Applications` icon; choose **Replace** if an older copy is installed
- Windows 10/11 (x64): use the `Setup.exe` installer (or the `.msi` package)

Before installing, verify your download against `checksums.txt`. Put it in the same folder as the download and run this there (`--ignore-missing` skips the builds for other platforms you did not download):

```bash
shasum -a 256 --ignore-missing -c checksums.txt
```

On Windows, run this in PowerShell and compare the hash with the matching line in `checksums.txt`:

```powershell
Get-FileHash .\HeiGate-Windows-x64-*-Setup.exe -Algorithm SHA256
```

The builds do not use an Apple Developer ID or Windows code-signing certificate yet. On macOS, use System Settings → Privacy & Security → Open Anyway if macOS cannot verify the developer. If macOS reports that the app is damaged, move HeiGate.app to Applications first, then remove the download quarantine. Windows SmartScreen may require More info → Run anyway.

```bash
xattr -dr com.apple.quarantine /Applications/HeiGate.app
```

## Connect your tools

Start HeiGate, add a channel in the UI, then copy the gateway URL and API key from Settings into your client. Manual setup works with any OpenAI-, Responses-, or Anthropic-compatible tool.

The gateway key shown in the UI authenticates clients. It is stored as `gateway.key` in the data directory (macOS: `~/Library/Application Support/HeiGate/gateway.key` with `0600` permissions; Windows: `%APPDATA%\HeiGate\gateway.key`).

**Claude Code**

```bash
export ANTHROPIC_BASE_URL="http://127.0.0.1:18080"
export ANTHROPIC_AUTH_TOKEN="<gateway key from the UI>"
```

**OpenAI-compatible clients** (Cursor, VS Code extensions, and so on)

- Base URL: `http://127.0.0.1:18080/v1`
- API key: `<gateway key from the UI>`

The gateway exposes `POST /v1/messages`, `POST /v1/chat/completions`, `POST /v1/responses`, and `GET /v1/models`.

## Build from source

You need Rust (stable) plus Node 22+ / pnpm 9+. Packaging on Linux also needs the WebKitGTK 4.1 and GTK 3 development libraries.

```bash
make app      # build the macOS HeiGate.app (Tauri)
open target/release/bundle/macos/HeiGate.app

make bundle   # build every bundle target for this platform (dmg / msi / deb / rpm / AppImage)
make run      # run the desktop app from source
make test     # Rust and frontend tests
make help     # list every target
```

For development it is easier to run the two halves separately:

```bash
make run                          # terminal 1: desktop gateway on a local port starting at 18080
pnpm --dir frontend run dev       # terminal 2: Vite on 5273, proxying to 18080
```

## Configuration

You normally don't need any. To relocate local data, use this environment variable:

| Variable | Default | Notes |
|---|---|---|
| `DATA_DIR` | Platform application data directory | Where the database and gateway key are stored |

## Security design

HeiGate opens an HTTP port on your machine, so it takes precautions:

- Only loopback connections are accepted
- The `Host` header must name the local machine, which defeats DNS rebinding
- The management API checks `Origin` and `Sec-Fetch-Site`, and every write additionally requires a custom request header — a browser cannot add custom headers to a cross-origin "simple" request, so other web pages cannot forge actions on your behalf
- Gateway endpoints require key authentication
- Channel credentials are masked in API responses unless explicitly requested
- A strict CSP; the UI loads nothing from the network

To report a vulnerability, see [SECURITY.md](SECURITY.md).

## Layout

```
crates/heigate-core    core library (server / api / store / router / resilience / protocol / probe / middleware)
src-tauri              Tauri 2 shell: starts the core, opens the window, bundles the UI
frontend               Vue 3 interface
```

## Notes

- Follow the terms of whichever AI providers you use. Relay or shared endpoints may violate some providers' agreements; that risk is yours to assess.
- This project only routes requests locally. It ships no API credits and no accounts.
- The project name and logo are unaffiliated with any provider and imply no endorsement.

## Acknowledgements

HeiGate is written independently. While designing the channel scheduling, failover, and probing approach for a local gateway, I studied the architecture of [sub2api](https://github.com/Wei-Shaw/sub2api) — thanks to that project for the ideas.

## License

[MIT](LICENSE) © 2026 Heiheiaoo
