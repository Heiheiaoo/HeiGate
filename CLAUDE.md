# HeiGate

本地 AI API 网关桌面应用（macOS arm64 / Windows x64 / Linux x64）。多渠道路由 + 故障转移 + 用量统计，数据只存本机，服务只监听回环地址。

- **仓库**：`github.com/Heiheiaoo/HeiGate-Desktop`
- **许可**：MIT
- **技术栈**：Rust（Tauri 2 外壳 + axum + rusqlite）/ Vue 3（仅依赖 vue 本体）
- **规模**：Rust 核心约 6.5k 行（含测试），前端 15k 行

## 常用命令

```bash
make help     # 列出所有目标
make run      # 从源码跑桌面应用（cargo run；数据在平台默认位置，可用 DATA_DIR 覆盖）
make test     # cargo test --workspace + 前端 typecheck/vitest
make lint     # cargo clippy -D warnings + eslint
make check    # 提交前完整门禁（与 CI 一致：fmt-check + clippy + 测试 + 前端构建）
make app      # 构建 macOS HeiGate.app（Tauri；产物在 target/release/bundle/macos）
make bundle   # 构建当前平台的全部打包产物（dmg/msi/deb/…）
```

前后端分开开发时：

```bash
make run                       # 终端 1：应用（网关默认 18080）
pnpm --dir frontend run dev    # 终端 2：Vite，5273，API 代理到 8080
```

提交前请跑 `make check`：它等价于 `cargo fmt --check` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace` + 前端 typecheck/lint/vitest/build，与 CI 保持一致。

## 目录结构

```
crates/heigate-core      核心库（不依赖 Tauri，可独立测试）
  src/server.rs          路由装配与启动（/health、/api/*、/v1/*、SPA fallback）
  src/api/               管理接口（handlers）+ 网关转发（gateway/forward/policy）
  src/store.rs           渠道/日志/设置的 SQLite 存储
  src/{router,resilience,canonical}.rs  路由、熔断、模型名归一化
  src/{probe,probe_inject,runner}.rs    探活、注入检测、探活调度
  src/protocol/          Anthropic ↔ OpenAI 协议转换，含流式
  src/middleware.rs      回环限制、DNS rebinding 与 CSRF 防护、CSP、网关认证
  src/{auth,datadir,version}.rs         网关密钥、数据目录、版本
src-tauri                Tauri 2 外壳：起核心服务、开窗口指向它、打包前端
frontend                 Vue 3 单页界面
```

## 编码约定

**依赖保持精简。** 核心只加了 axum / tokio / reqwest / rusqlite / serde / regex / chrono；前端运行时只有 Vue。加新依赖前先想清楚是否真的必要。

**注释写「为什么」，不写「做了什么」。** 代码本身能说明后者。

**动 `crates/heigate-core/src/middleware.rs` 要格外小心。** 那是本地服务的安全边界，任何放宽都要能说清理由。

## 几个关键设计（改代码前最好先读）

**故障转移卡在首字节。** `api/gateway.rs` 的 `forward` 在读上游响应时先取一段前缀：SSE 读到帧边界（`\n\n`）且出现错误帧或输出信号，非 SSE 读至少 1 字节。首字节之前失败可以安全切换渠道重试，一旦有内容发给了客户端就不能再重放，否则会出现重复输出。读到的前缀用 `stream::once(...).chain(rest)` 接回流里，不会丢数据。这是最容易改错的地方。

**熔断是惰性恢复的。** 不起定时器。报过错的渠道冷却期过后，第一个请求会把它置为 `half_open`，排到候选最前面放一个真实请求当探针（同一时刻只放一个），成功就关闭熔断，失败重新冷却。所以每类失败都必须有非零冷却，否则坏渠道会被每个请求优先尝试。冷却中的渠道只在没有别的渠道可用时兜底。失败分类和冷却时长见 `src/resilience.rs`（认证错误 10 分钟、模型不存在 2 分钟、有 `Retry-After` 就按它（封顶 1 小时），限流、网络和 5xx 指数退避 15s→5min 封顶）。

**上游错误按策略判定，不猜形状。** `src/api/policy.rs` 的 `upstream_error_payload` 只认两种铁证：非空 `error` 值，或顶层 `code` 为 4xx/5xx；空槽位（`""`、`{}`、`[]`、`{"type":"","message":""}`）都不算错误，裸 `message` 也不算。误判会把健康渠道判失败并熔断。

**探活只手动触发，发算术题。** 后台不做定时探活，渠道恢复靠上面的 half-open 真实请求，不花额外额度。只 ping 端点或调 `/v1/models` 无法证明「这个密钥能用这个模型跑推理」。手动探活只执行一次，用户再次点击就是下一次独立采样；默认只查主模型，额外模型由用户显式触发队列检查。两位数加法能一次覆盖端点、密钥、模型名、额度；探活读完整个（被截短的）流，以免首个 delta 之后的流内错误被当成健康。达到 6 秒记 degraded，没有任何文本或 reasoning 记 failed，HTTP/网络错误记 error。不校验算术答案。

**协议转换的坑位：**
- `tool_result` 必须紧跟发起调用的 assistant 消息（OpenAI 要求），而 Anthropic 把它放在 user turn 里，转换时要提出来排前面
- assistant 只有工具调用时整个 `content` 字段省略，有些上游会因 `content: ""` 报错
- thinking 块不回放（签名是供应商特定的）
- 流式切换内容类型（thinking → text → tool_use）时要关旧块开新块，`StreamState` 负责记账
- 上游不给 usage 时按字符估算（CJK 1 字符 1 token，其他 4 字符 1 token），拿到真实值立即覆盖

**SQLite 用单连接。** `store.rs` 把连接放在 `Mutex` 里串行化，让探活更新和日志写入顺序确定。schema 迁移用 `CREATE TABLE IF NOT EXISTS` + 逐条 `ALTER TABLE`（忽略「列已存在」错误），没有迁移框架；时间戳存 Go 兼容的 RFC3339Nano 字符串。

**前端刻意不用路由和全局状态管理。** 只有一个页面，主要状态仍由 `GatewayView.vue` 管理；渠道卡片、表格、工具栏和模型编辑器位于 `components/channels/`，`services/` 负责跟后端通信。日志实时更新走 SSE（`GET /api/logs/stream`），不是轮询。

**外壳的托盘与自启动。** `src-tauri` 建了托盘图标与菜单（显示主窗口 / 开机自启 / 退出），关闭窗口只是隐藏、进程继续服务，退出要在托盘里显式选。开机自启用 `tauri-plugin-autostart`（macOS 走 LaunchAgent）。

**外壳与前端是「本地 HTTP + WebView」。** `src-tauri` 起 `heigate-core` 的 axum 服务（回环端口），窗口直接指向 `http://127.0.0.1:<port>`，所以前端用相对路径调 API，跟前端契约完全一致。构建出的前端作为 Tauri 资源打进 `HeiGate.app/Contents/Resources/dist`。

## 安全模型

网关在本机开 HTTP 端口，主要防的是**用户浏览器里打开的任意网页**（它们都能向 `127.0.0.1` 发请求）。三层防护：

1. 只接受回环连接
2. 校验 `Host` 头必须是本机（防 DNS rebinding）
3. 管理接口检查 `Origin` / `Sec-Fetch-Site`，所有写操作要求 `X-HeiGate-Request` 头——浏览器无法在跨源「简单请求」里带自定义头

网关端点（`/v1/*`）用 `gateway.key` 做 Bearer 认证，密钥文件权限 `0600`。渠道 API Key 在接口返回时打码，只有显式 `include_secrets` 才返回完整值。

前端目前不使用 `v-html`，所有动态文本都走 `{{ }}` 插值（Vue 自动转义）。若将来确需 `v-html`，插值处必须先经 `escapeHtml()`。

## 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `HEIGATE_PORT` | 18080，被占用时顺延到 18090 再随机 | 监听端口（源码里保留 flags 兼容） |
| `DATA_DIR` | macOS `~/Library/Application Support/HeiGate`，Windows `%APPDATA%\HeiGate`，Linux `~/.local/share/heigate` | 数据库和网关密钥位置 |
| `HEIGATE_STATIC_DIR` | 打包资源目录 | 覆盖前端静态目录（调试用） |

## 发布

更新根目录 `VERSION`，然后打 tag：

```bash
git tag v1.0.0 && git push origin v1.0.0
```

`.github/workflows/release.yml` 会并行构建三平台产物（macOS arm64 `.dmg`/`.zip`、Windows x64 `.msi`/`.exe`、Linux x64 `.deb`/`.rpm`/`.AppImage`），统一生成 checksums 并创建 Release。所有产物目前都未签名，首次打开需要用户在「隐私与安全性」或 SmartScreen 里放行。

已发布的 tag 不要移动或覆盖产物：别人可能已经下载，checksums 也会对不上。有问题就发下一个补丁版本。

## 已知待办

- 未使用 Apple Developer ID / Windows 代码签名证书，首次打开需要用户手动放行
- 已覆盖渠道工具栏、模型管理、焦点陷阱、日志详情和诊断内容组件；完整 GatewayView 页面级交互仍缺少测试
- `GatewayView.vue` 仍然较大；新增独立交互时优先继续拆到对应功能组件
- 探活和网关目前只支持 Anthropic 和 OpenAI 两种 provider，Gemini 等需要新增 adapter

## 更多

- 实现细节见 `docs/DEVELOPMENT.md`
- 协作约定见 `CONTRIBUTING.md`
- 威胁模型见 `SECURITY.md`

项目独立编写；设计本地网关的调度、故障转移和探活思路时参考了 [sub2api](https://github.com/Wei-Shaw/sub2api) 的架构。
