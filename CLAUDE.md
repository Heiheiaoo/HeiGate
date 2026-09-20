# HeiGate

macOS 本地 AI API 网关。多渠道路由 + 故障转移 + 用量统计，数据只存本机，服务只监听回环地址。

- **Module**：`github.com/Heiheiaoo/heigate`
- **许可**：MIT
- **技术栈**：Go 1.27（Gin + 纯 Go SQLite）/ Vue 3（仅依赖 vue 本体）/ Swift（Cocoa + WebKit 外壳）
- **规模**：Go 约 8.5k 行（含 1.6k 测试），前端 13k 行，Swift 709 行，共 64 个文件

## 常用命令

```bash
make help     # 列出所有目标
make run      # 从源码跑网关，数据落在 ./desktop-data
make test     # 后端 go test + 前端 typecheck/vitest
make lint     # go vet + eslint
make app      # 构建 dist/HeiGate.app（需要 Xcode Command Line Tools）
```

前后端分开开发时：

```bash
make run                       # 终端 1：网关，8080
pnpm --dir frontend run dev    # 终端 2：Vite，5273，API 代理到 8080
```

提交前请跑：`gofmt -w ./cmd ./internal && go vet ./... && go test ./... && pnpm --dir frontend run typecheck && pnpm --dir frontend run lint`

## 目录结构

```
cmd/heigate         入口：装配路由、启动 HTTP 服务
internal/api        本地管理接口（/api/*）+ 网关端点（/v1/*）
internal/channel    渠道 SQLite 存储、路由与故障转移、熔断、探活调度、模型名归一化
internal/probe      上游健康探活（独立包，无外部依赖，易测）
internal/protocol   Anthropic ↔ OpenAI 协议转换，含流式（独立包，易测）
internal/middleware 回环限制、DNS rebinding 与 CSRF 防护、CSP、网关认证
internal/datadir    数据目录解析（DATA_DIR 或平台默认位置）
internal/web        embed 编译好的前端（-tags embed）
frontend            Vue 3 单页界面
desktop/macos       Swift 原生外壳
```

## 编码约定

**命名冲突要当心。** `internal/api/gateway.go` 里 `channel` 和 `protocol` 都是包名，所以：

- 渠道变量一律用 `ch`，不要用 `channel`
- 协议变量一律用 `proto`，不要用 `protocol`

**依赖保持精简。** 后端目前只有 Gin + `modernc.org/sqlite`（纯 Go，无需 CGO），前端运行时只有 Vue。加新依赖前先想清楚是否真的必要。

**注释写「为什么」，不写「做了什么」。** 代码本身能说明后者。

**动 `internal/middleware` 要格外小心。** 那是本地服务的安全边界，任何放宽都要能说清理由。

## 几个关键设计（改代码前最好先读）

**故障转移卡在首字节。** `forwardUpstream` 会先读 4 KB 前缀：首字节之前失败可以安全切换渠道重试，一旦有内容发给了客户端就不能再重放，否则会出现重复输出。前缀用 `io.MultiReader` 接回流里，不会丢数据。这是最容易改错的地方。

**熔断是惰性恢复的。** 不起定时器。冷却期过后第一个请求会把渠道置为 `half_open` 放行当探针，成功就关闭熔断，失败重新打开。失败分类和冷却时长见 `internal/channel/resilience.go`（认证错误 10 分钟、模型不存在 2 分钟、限流看 `Retry-After`、网络和 5xx 指数退避 15s→5min 封顶）。

**探活发算术题。** 只 ping 端点或调 `/v1/models` 无法证明「这个密钥能用这个模型跑推理」。周期任务只检查主模型，每个时间点只尝试一次，失败交给下一周期恢复；手动探活也只执行一次，用户再次点击就是下一次独立采样。额外模型由用户显式触发队列检查，避免稳定渠道反复消耗所有模型额度。两位数加法能一次覆盖端点、密钥、模型名、额度；达到 6 秒记 degraded，没有任何文本或 reasoning 记 failed，HTTP/网络错误记 error。答案匹配只作为诊断提示，不作为可用性硬判据。

**协议转换的坑位：**
- `tool_result` 必须紧跟发起调用的 assistant 消息（OpenAI 要求），而 Anthropic 把它放在 user turn 里，转换时要提出来排前面
- assistant 只有工具调用时整个 `content` 字段省略，有些上游会因 `content: ""` 报错
- thinking 块不回放（签名是供应商特定的）
- 流式切换内容类型（thinking → text → tool_use）时要关旧块开新块，`StreamState` 负责记账
- 上游不给 usage 时按字符估算（CJK 1 字符 1 token，其他 4 字符 1 token），拿到真实值立即覆盖

**SQLite 连接池限制为 1。** 桌面写入量小，串行化让探活更新和日志写入顺序确定。schema 迁移用 `CREATE TABLE IF NOT EXISTS` + 逐条 `ALTER TABLE`（忽略「列已存在」错误），没有迁移框架。

**前端刻意不用路由和全局状态管理。** 只有一个页面，主要状态仍由 `GatewayView.vue` 管理；渠道卡片、表格、工具栏和模型编辑器位于 `components/channels/`，`services/` 负责跟后端通信。日志实时更新走 SSE（`GET /api/logs/stream`），不是轮询。

## 安全模型

网关在本机开 HTTP 端口，主要防的是**用户浏览器里打开的任意网页**（它们都能向 `127.0.0.1` 发请求）。三层防护：

1. 只接受回环连接
2. 校验 `Host` 头必须是本机（防 DNS rebinding）
3. 管理接口检查 `Origin` / `Sec-Fetch-Site`，所有写操作要求 `X-HeiGate-Request` 头——浏览器无法在跨源「简单请求」里带自定义头

网关端点（`/v1/*`）用 `gateway.key` 做 Bearer 认证，密钥文件权限 `0600`。渠道 API Key 在接口返回时打码，只有显式 `include_secrets` 才返回完整值。

诊断建议用了 `v-html`，插值处必须走 `escapeHtml()`。

## 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `HEIGATE_HOST` | `127.0.0.1` | 监听地址。改成非回环会关闭本地防护 |
| `HEIGATE_PORT` | `8080` | 监听端口 |
| `DATA_DIR` | `~/Library/Application Support/HeiGate` | 数据库和网关密钥位置 |

## 发布

更新根目录 `VERSION`，然后打 tag：

```bash
git tag v0.1.0 && git push origin v0.1.0
```

`.github/workflows/release.yml` 会在 macOS runner 上构建 `.app`、打包 `.dmg` / `.zip`、生成 checksums 并创建 Release。产物未签名未公证。

## 已知待办

- 应用未做代码签名和公证，首次打开需要用户在「隐私与安全性」里放行
- 仅支持 Apple 芯片（arm64），没有 Intel 构建
- `GatewayView.vue` 仍然较大；新增独立交互时优先继续拆到对应功能组件，避免把已经抽离的渠道 UI 再塞回主页面
- 前端只有 API 层的 3 个测试，UI 交互没有测试覆盖
- 探活和网关目前只支持 Anthropic 和 OpenAI 两种 provider，Gemini 等需要新增 adapter

## 更多

- 实现细节见 `docs/DEVELOPMENT.md`
- 协作约定见 `CONTRIBUTING.md`
- 威胁模型见 `SECURITY.md`

项目独立编写；设计本地网关的调度、故障转移和探活思路时参考了 [sub2api](https://github.com/Wei-Shaw/sub2api) 的架构。
