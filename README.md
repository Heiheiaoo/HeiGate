<div align="center">

<img src="assets/logo.svg" alt="HeiGate Logo" width="128" />

# HeiGate

[![Go](https://img.shields.io/badge/Go-1.27.0-00ADD8.svg)](https://golang.org/)
[![Vue](https://img.shields.io/badge/Vue-3.4+-4FC08D.svg)](https://vuejs.org/)

**本地优先的 macOS 桌面 AI API 网关，提供多渠道路由、故障转移、用量管理和客户端集成。**

中文 | [English](README_EN.md)

</div>

HeiGate 是面向 macOS 的本地桌面 AI API 网关。数据保存在本地 SQLite 数据库中，提供渠道探活、模型路由、请求分析和 Claude/Codex 客户端配置。

> [!IMPORTANT]
> HeiGate 基于 [Wei-Shaw/sub2api](https://github.com/Wei-Shaw/sub2api) 修改而来，不是 sub2api 官方仓库，由独立维护者维护。版权和许可说明见 [NOTICE](NOTICE)。

## 快速开始

### 安装已打包应用（推荐）

从 GitHub Releases 页面下载最新的安装包：

```bash
open HeiGate-macOS-arm64-*.dmg
```

GitHub Release 页面提供 `.dmg`、`.zip` 和校验文件。当前安装包面向 Apple 芯片 Mac 构建。

### 从源码构建

```bash
make build-desktop-macos
open dist/desktop/HeiGate.app
```

## 开源社区

- 提交 Pull Request 前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md)。
- 报告问题或提议功能时，请使用 GitHub Issue 模板。
- 安全漏洞请按 [SECURITY.md](SECURITY.md) 私下报告。
- 社区行为遵循 [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)。
- 发布与兼容性约定见 [docs/OPEN_SOURCE.md](docs/OPEN_SOURCE.md)。


## ⚠️ 重要提醒

使用本项目前，请务必仔细阅读以下内容：

- **🚨 服务条款风险**：使用本项目可能违反 Anthropic 等上游服务商的服务条款。请在使用前仔细阅读相关服务商的用户协议，由此产生的一切风险由用户自行承担。
- **⚖️ 合规使用**：请在符合您所在国家或地区法律法规的前提下使用本项目，严禁将其用于任何违法违规用途。
- **📖 免责声明**：本项目仅供技术学习与研究使用，作者不对因使用本项目导致的账户封禁、服务中断、数据丢失或其他任何直接或间接损失承担责任。
- **🏷️ 不授予商标或服务背书**：LGPL 许可证允许在其条款下使用本项目，包括商业使用；但不授予项目名称、Logo、上游账号、托管服务或服务商关系的任何权利，也不代表维护者背书。
- **🤝 独立维护说明**：除明确标注外，本文档中的功能、配置和链接仅用于说明 HeiGate，不代表与任何第三方存在赞助、合作、代理或推荐关系。


## 项目概述

HeiGate 是一个 macOS 桌面 AI API 网关，用于管理本地上游渠道并为编程客户端路由请求。网关凭据和请求数据默认保存在本机。

## 核心功能

- **多账号管理** - 支持多种上游账号类型（OAuth、API Key）
- **本地 API Key** - 为编程客户端生成和管理本地 API Key
- **请求分析** - Token 级别用量追踪和请求分析
- **智能调度** - 智能账号选择，支持粘性会话
- **并发控制** - 用户级和账号级并发限制
- **速率限制** - 可配置的请求和 Token 速率限制
- **管理后台** - Web 界面进行监控和管理
- **桌面打包** - GitHub Actions 自动构建 Apple 芯片 `.dmg` 和 `.zip` 安装包


## 技术栈

| 组件 | 技术 |
|------|------|
| 后端 | Go 1.27.0, Gin, Ent |
| 前端 | Vue 3.4+, Vite 5+, TailwindCSS |
| 数据库 | SQLite（本地桌面数据） |
| 桌面外壳 | macOS Cocoa + WebKit |

---

## Antigravity 使用说明

HeiGate 支持 [Antigravity](https://antigravity.so/) 账户，授权后可通过专用端点访问 Claude 和 Gemini 模型。

### 专用端点

| 端点 | 模型 |
|------|------|
| `/antigravity/v1/messages` | Claude 模型 |
| `/antigravity/v1beta/` | Gemini 模型 |

### Claude Code 配置示例

```bash
export ANTHROPIC_BASE_URL="http://localhost:8080/antigravity"
export ANTHROPIC_AUTH_TOKEN="sk-xxx"
```

### 混合调度模式

Antigravity 账户支持可选的**混合调度**功能。开启后，通用端点 `/v1/messages` 和 `/v1beta/` 也会调度该账户。

> **⚠️ 注意**：Anthropic Claude 和 Antigravity Claude **不能在同一上下文中混合使用**，请通过分组功能做好隔离。

---

## 项目结构

```
HeiGate/
├── backend/                  # Go 后端服务
│   ├── cmd/server/           # 应用入口
│   ├── internal/             # 内部模块
│   │   ├── config/           # 配置管理
│   │   ├── model/            # 数据模型
│   │   ├── service/          # 业务逻辑
│   │   ├── handler/          # HTTP 处理器
│   │   └── gateway/          # API 网关核心
│   └── resources/            # 静态资源
│
├── frontend/                 # Vue 3 前端
│   └── src/
│       ├── api/              # API 调用
│       ├── stores/           # 状态管理
│       ├── views/            # 页面组件
│       └── components/       # 通用组件
│
├── desktop/macos/            # 原生 macOS 应用外壳
└── .github/workflows/        # 自动桌面应用发布流程
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

## 许可证与归属

本项目基于 [GNU 宽通用公共许可证 v3.0](LICENSE)（或更高版本）授权。

HeiGate 包含基于 [Wei-Shaw/sub2api](https://github.com/Wei-Shaw/sub2api) 修改的作品。原始版权归各自权利人所有，详见 [NOTICE](NOTICE)。

---

<div align="center">

**如果觉得有用，请给个 Star 支持一下！**

</div>
