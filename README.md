<div align="center">

<img src="assets/logo.svg" alt="HeiGate" width="120" />

# HeiGate

**一个运行在你自己电脑上（macOS / Windows）的 AI API 网关。多渠道路由、自动故障转移、用量统计，数据不出本机。**

[![CI](https://github.com/Heiheiaoo/HeiGate/actions/workflows/ci.yml/badge.svg)](https://github.com/Heiheiaoo/HeiGate/actions/workflows/ci.yml)
[![Vue](https://img.shields.io/badge/Vue-3-4FC08D.svg)](https://vuejs.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

中文 | [English](README.en.md)

</div>

## 这是什么

你手上可能有好几个 AI API 渠道：官方的、中转的、自己搭的 Ollama。HeiGate 把它们收拢到一个本地地址后面，你的编程工具只需要连桌面版默认地址 `http://127.0.0.1:28888`，剩下的事情它来处理：

- **多渠道路由** — 按延迟、质量评分或轮询挑选渠道
- **自动故障转移** — 某个渠道报错、限流或超时，请求自动切到下一个；连续失败会触发熔断和冷却
- **协议互通** — 用 Anthropic 的客户端调 OpenAI 兼容的上游，请求和响应（含流式）实时转换
- **健康探活** — 手动触发，用渠道列表中的首个模型执行轻量算术题，需要时再队列检查全部模型，验证密钥、端点、模型名是否真的可用；不在后台定时消耗额度
- **用量统计** — 每次请求的渠道、模型、token、首字延迟、失败链路都记在本地，可按天/模型/渠道汇总
- **Hermes 接入** — 在应用中配置 Hermes，并通过本地网关调度供应商

数据保存在系统用户目录下的 SQLite 文件里（macOS：`~/Library/Application Support/HeiGate`，Windows：`%APPDATA%\HeiGate`），不上传任何地方。网关只监听回环地址。

## 安装

从 [Releases](https://github.com/Heiheiaoo/HeiGate/releases) 下载对应版本：

- Apple 芯片 Mac（arm64）：先退出 HeiGate，打开 `.dmg`，把 HeiGate 拖到「Applications」图标；如果提示已有旧版本，选择「替换」
- Windows 10/11（x64）：下载 `Setup.exe` 安装（或使用 `.msi` 安装包）

安装前建议先用 `checksums.txt` 校验下载的文件。把它和安装包放在同一目录，然后在该目录执行（`--ignore-missing` 会跳过你没有下载的其他平台文件）：

```bash
shasum -a 256 --ignore-missing -c checksums.txt
```

Windows 上在 PowerShell 里执行下面的命令，把输出的哈希与 `checksums.txt` 里对应的一行比对：

```powershell
Get-FileHash .\HeiGate-Windows-x64-*-Setup.exe -Algorithm SHA256
```

安装包暂未使用 Apple Developer ID / Windows 代码签名。macOS 首次打开时如果提示无法验证开发者，请在「系统设置 → 隐私与安全性」点「仍要打开」；如果系统显示“已损坏”，先把 HeiGate.app 拖进「应用程序」，再执行下面的命令移除下载隔离属性。Windows SmartScreen 可能需要点「更多信息 → 仍要运行」。

```bash
xattr -dr com.apple.quarantine /Applications/HeiGate.app
```

## 连接你的工具

启动 HeiGate 后，在界面里添加供应商。网关配置页顶部显示统一地址和 Key；将它们填入需要连接的工具，所有请求共用同一套供应商调度规则。

默认网关地址为 `http://127.0.0.1:28888/v1`。网关密钥保存在数据目录下的 `gateway.key`（macOS：`~/Library/Application Support/HeiGate/gateway.key`，权限 `0600`；Windows：`%APPDATA%\HeiGate\gateway.key`）。
- API Key：`<界面里的网关密钥>`

网关提供统一的 `/v1` API：`POST /v1/messages`、`POST /v1/chat/completions`、`POST /v1/responses` 和 `GET /v1/models`。所有接口使用同一套网关调度配置。

## 从源码构建

需要 Rust（stable）与 Node 22+ / pnpm 9+。Linux 打包还需要 WebKitGTK 4.1 与 GTK 3 开发库。

```bash
make app      # 构建 macOS HeiGate.app（Tauri）
open target/release/bundle/macos/HeiGate.app

make bundle   # 构建当前平台的全部打包产物（dmg / msi / deb / rpm / AppImage）
make run      # 直接从源码跑桌面应用
make test     # Rust + 前端测试
make help     # 看所有目标
```

开发时前后端分开跑更方便：

```bash
make run                          # 终端 1：桌面网关，监听 28888 起的本地端口
pnpm --dir frontend run dev       # 终端 2：Vite，5273，API 请求代理到 28888
```

## 配置

一般不需要改。数据目录需要时可以用环境变量：

| 变量 | 默认值 | 说明 |
|---|---|---|
| `DATA_DIR` | 平台应用数据目录 | 数据库和网关密钥的存放位置 |

## 安全设计

HeiGate 在你的机器上开了一个 HTTP 端口，所以做了这些约束：

- 只接受来自回环地址的连接
- 校验 `Host` 头必须是本机，防止 DNS rebinding
- 管理接口检查 `Origin` 和 `Sec-Fetch-Site`，所有写操作还要求一个自定义请求头 —— 浏览器无法在跨源的"简单请求"里带上自定义头，因此其他网页无法伪造你的操作
- 网关端点要求密钥认证
- 渠道密钥在接口返回时打码，只有显式请求才返回完整值
- 严格的 CSP，界面不加载任何外部资源

发现安全问题请看 [SECURITY.md](SECURITY.md)。

## 项目结构

```
crates/heigate-core    核心库（server / api / store / router / resilience / protocol / probe / middleware）
src-tauri              Tauri 2 外壳：起核心服务、开窗口、打包前端
frontend               Vue 3 界面
```

## 说明

- 使用第三方 AI 服务时请遵守对应服务商的条款。用中转或共享渠道可能违反某些服务商的协议，风险由使用者自行承担。
- 本项目仅提供本地路由能力，不附带任何 API 额度或账号。
- 项目名称、Logo 与相关服务商无关，也不代表任何背书或合作关系。

## 参考与致谢

HeiGate 是独立编写的项目。在设计本地网关的渠道调度、故障转移和探活思路时，参考了 [sub2api](https://github.com/Wei-Shaw/sub2api) 的架构设计，在此致谢。

## 许可

[MIT](LICENSE) © 2026 Heiheiaoo
