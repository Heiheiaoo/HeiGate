# 开发笔记

这份文档记录一些不适合放进 README 的实现细节。

## 请求的完整路径

以一次 `POST /v1/messages` 为例：

1. `internal/middleware.LoopbackOnly` 确认连接来自本机、`Host` 是本机地址
2. `internal/middleware.BearerAuth` 校验网关密钥
3. `internal/api.forward` 读取请求体，解析出模型名
4. `resolveModelAlias` 应用自定义模型映射；若无渠道支持该模型，会退回到任一可用渠道的主模型
5. `internal/channel.Router.CandidatesMatching` 过滤出支持该模型且协议匹配的渠道，按健康度分层（healthy → degraded → failed → cooling down），再按策略排序
6. 先写一条状态为「传输中」的日志，让界面能实时看到
7. `RouteCandidates` 依次尝试候选渠道，每个渠道最多重试一次
8. `forwardUpstream` 判断是否需要协议转换：客户端说 Anthropic 而上游是 OpenAI 时走 `forwardTranslated`，否则直接转发并改写模型名
9. **首字节之后不再故障转移** —— 已经发给客户端的内容不能重放，否则会出现重复输出
10. 边转发边统计 token，定期更新日志；结束时写入最终状态

## 为什么故障转移要卡在首字节

这是最容易写错的地方。如果上游已经吐了一部分内容给客户端，此时再切换渠道重试，客户端会收到两段拼接的回答。所以 `forwardUpstream` 先读 4 KB 前缀：读到内容之前失败可以安全重试，读到之后就只能把错误透传出去。前缀会用 `io.MultiReader` 接回流里，不会丢。

## 熔断

`internal/channel/resilience.go` 把失败分类，不同类型冷却时间不同：认证错误 10 分钟（重试没意义），模型不存在 2 分钟；上游给了 `Retry-After` 就按它来（封顶 1 小时）；限流、网络和 5xx 走指数退避（15 秒起，上限 5 分钟）。连续三次可熔断的失败会把渠道置为 open。

恢复是惰性的：不起定时器。报过错的渠道冷却期过后，第一个请求到来时会把它改成 half_open，排到候选最前面放一个真实请求过去当探针，同一时刻只放一个。成功就关闭熔断，失败则重新冷却。正因为恢复后会被优先尝试，每类失败都必须有非零冷却，否则一个持续报错（比如一直 429）的渠道会被每个请求先打一遍。冷却中的渠道不会被丢弃，没有别的渠道可用时仍会兜底。

## 探活为什么用算术题

只 ping 端点或调 `/v1/models` 都不能证明「这个密钥能用这个模型跑推理」。发一道两位数加法能一次性覆盖端点、密钥、模型名、额度四件事。探活只由用户手动触发，后台不定时探活，避免持续消耗额度；渠道的自动恢复交给上面的 half-open 真实请求。每次探活只执行一次，用户再次点击就是下一次独立采样。默认只查决定渠道状态的主模型，额外模型由界面的全模型队列按需检查。探活会读完整个被截短的流，这样首个 delta 之后才出现的流内错误也能被识别。响应达到 6 秒记为 degraded，没有产生任何文本或 reasoning 记为 failed，HTTP/网络错误记为 error。不校验算术答案：数字经常被拆进多个 delta，生成长度又被刻意压得很短，截断是常态，不能据此把可用渠道判死。

## 协议转换的注意点

- **tool_result 必须紧跟 tool_calls。** OpenAI 要求 `tool` 角色的消息紧随发起调用的 assistant 消息。Anthropic 把 tool_result 放在 user turn 里，所以转换时要把它们从 user turn 中提出来、排在前面。
- **assistant 只有工具调用时不能带 content 字段。** 有些上游会因为 `content: ""` 报错，所以那种情况下整个字段省略。
- **thinking 块不回放。** 转成 OpenAI 请求时丢弃，因为签名是特定供应商的，回放没有意义。
- **流式的块索引。** Anthropic 的 `content_block_*` 事件带 index，且切换内容类型（thinking → text → tool_use）时要关旧块开新块。`StreamState` 负责这个记账。
- **usage 兜底。** 上游不给 usage 时用字符数估算（CJK 字符按 1 token，其他按 4 字符 1 token），拿到真实 usage 后立即改用真实值。

## SQLite 的取舍

`internal/channel/store.go` 把连接池限制为 1 个连接。桌面场景写入量很小，串行化让探活更新和日志写入的顺序变得确定，省掉了一堆并发问题。schema 用 `CREATE TABLE IF NOT EXISTS` 加逐条 `ALTER TABLE`（忽略「列已存在」错误）来做迁移，不需要迁移框架。

## 前端

界面保持单页结构，没有引入前端路由和全局状态管理；页面状态主要由 `GatewayView.vue` 管理，渠道卡片、表格、工具栏和模型编辑器拆在 `components/channels/`，`services/` 负责和后端通信。运行时依赖只有 Vue 本身。

日志实时更新走 SSE（`GET /api/logs/stream`），不是轮询。

## 发布

打 tag 推上去即可：

```bash
git tag v1.0.0
git push origin v1.0.0
```

`.github/workflows/release.yml` 会并行构建 macOS arm64 的 `.dmg` / `.zip` 和 Windows x64 的 `Setup.exe` / 便携 `.zip`，最后统一生成 checksums 并创建 Release。记得同步更新根目录的 `VERSION` 文件。
