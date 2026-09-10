<div align="center">

<img src="assets/logo.svg" alt="HeiGate Logo" width="128" />

# HeiGate

[![Go](https://img.shields.io/badge/Go-1.27.0-00ADD8.svg)](https://golang.org/)
[![Vue](https://img.shields.io/badge/Vue-3.4+-4FC08D.svg)](https://vuejs.org/)

**Local-first macOS desktop AI API gateway with multi-provider routing, failover, usage management, and client integration.**

English | [中文](README_CN.md) | [日本語](README_JA.md)

</div>

HeiGate is a local-first desktop AI API gateway for macOS. It stores data in a local SQLite database and includes channel health checks, model routing, request analytics, and Claude/Codex client configuration.

> [!IMPORTANT]
> HeiGate is a derivative of [Wei-Shaw/sub2api](https://github.com/Wei-Shaw/sub2api). It is not the official sub2api repository and is maintained independently. See [NOTICE](NOTICE) for attribution and licensing details.

## Quick Start

### macOS desktop application

```bash
make build-desktop-macos
open dist/desktop/HeiGate.app
```

### Install the packaged application

```bash
open HeiGate-macOS-arm64-*.dmg
```

The GitHub Release page contains `.dmg`, `.zip`, and checksum files. The macOS package is currently built for Apple silicon.

### Build from source

```bash
make build-desktop-macos
open dist/desktop/HeiGate.app
```

## Community

- Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.
- Use the issue templates for reproducible bug reports and focused feature proposals.
- Report vulnerabilities privately according to [SECURITY.md](SECURITY.md).
- Community conduct is governed by [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
- Release and compatibility expectations are documented in [docs/OPEN_SOURCE.md](docs/OPEN_SOURCE.md).

## ⚠️ Important Notice

Please read the following carefully before using this project:

- **🚨 Terms of Service Risk**: Using this project may violate the terms of service of Anthropic and other upstream providers. Please review the relevant providers' user agreements before use; all risks arising from such use are borne solely by the user.
- **⚖️ Compliant Use**: Use this project only in compliance with the laws and regulations of your country or region. Any unlawful use is strictly prohibited.
- **📖 Disclaimer**: This project is provided for technical learning and research purposes only. The authors assume no liability for account bans, service interruptions, data loss, or any other direct or indirect damages resulting from the use of this project.
- **🏷️ No Trademark Or Service Endorsement**: The LGPL license permits use under its terms, including commercial use. It does not grant rights to project names, logos, upstream accounts, hosted services, or provider relationships, and it does not imply endorsement by the maintainers.
- **🤝 Independent Maintenance**: Unless explicitly stated otherwise, features, configuration examples, and links in this documentation describe HeiGate only and do not imply sponsorship, partnership, agency, or endorsement by any third party.


## Overview

HeiGate is a macOS desktop AI API gateway for managing local upstream channels and routing requests from coding clients. It keeps gateway credentials and request data on the local machine.

## Features

- **Multi-Account Management** - Support multiple upstream account types (OAuth, API Key)
- **Local API Keys** - Generate and manage local API keys for coding clients
- **Usage Analytics** - Token-level usage tracking and request analysis
- **Smart Scheduling** - Intelligent account selection with sticky sessions
- **Concurrency Control** - Per-user and per-account concurrency limits
- **Rate Limiting** - Configurable request and token rate limits
- **Admin Dashboard** - Web interface for monitoring and management
- **Desktop Packaging** - Automated GitHub Releases build `.dmg` and `.zip` packages for Apple silicon


## Tech Stack

| Component | Technology |
|-----------|------------|
| Backend | Go 1.27.0, Gin, Ent |
| Frontend | Vue 3.4+, Vite 5+, TailwindCSS |
| Database | SQLite (local desktop data) |
| Desktop shell | macOS Cocoa + WebKit |


## Antigravity Support

HeiGate supports [Antigravity](https://antigravity.so/) accounts. After authorization, dedicated endpoints are available for Claude and Gemini models.

### Dedicated Endpoints

| Endpoint | Model |
|----------|-------|
| `/antigravity/v1/messages` | Claude models |
| `/antigravity/v1beta/` | Gemini models |

### Claude Code Configuration

```bash
export ANTHROPIC_BASE_URL="http://localhost:8080/antigravity"
export ANTHROPIC_AUTH_TOKEN="sk-xxx"
```

### Hybrid Scheduling Mode

Antigravity accounts support optional **hybrid scheduling**. When enabled, the general endpoints `/v1/messages` and `/v1beta/` will also route requests to Antigravity accounts.

> **⚠️ Warning**: Anthropic Claude and Antigravity Claude **cannot be mixed within the same conversation context**. Use groups to isolate them properly.

---

## Project Structure

```
HeiGate/
├── backend/                  # Go backend service
│   ├── cmd/server/           # Application entry
│   ├── internal/             # Internal modules
│   │   ├── config/           # Configuration
│   │   ├── model/            # Data models
│   │   ├── service/          # Business logic
│   │   ├── handler/          # HTTP handlers
│   │   └── gateway/          # API gateway core
│   └── resources/            # Static resources
│
├── frontend/                 # Vue 3 frontend
│   └── src/
│       ├── api/              # API calls
│       ├── stores/           # State management
│       ├── views/            # Page components
│       └── components/       # Reusable components
│
├── desktop/macos/            # Native macOS application shell
└── .github/workflows/        # Automated desktop release workflow
```

## Star History

<a href="https://star-history.dera.page/#Heiheiaoo/HeiGate&Date">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://star-history.dera.page/svg?repos=Heiheiaoo/HeiGate&type=Date&theme=dark" />
   <source media="(prefers-color-scheme: light)" srcset="https://star-history.dera.page/svg?repos=Heiheiaoo/HeiGate&type=Date" />
   <img alt="Star History Chart" src="https://star-history.dera.page/svg?repos=Heiheiaoo/HeiGate&type=Date" />
 </picture>
</a>

---

## License And Attribution

This project is licensed under the [GNU Lesser General Public License v3.0](LICENSE) (or later).

HeiGate contains work derived from [Wei-Shaw/sub2api](https://github.com/Wei-Shaw/sub2api). Original copyright notices remain with their respective owners. See [NOTICE](NOTICE).

---

<div align="center">

**If you find this project useful, please give it a star!**

</div>
