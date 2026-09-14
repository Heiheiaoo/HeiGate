# 参与开发

欢迎提 issue 和 PR。

## 环境

- Go 1.27+
- Node 22+ 与 pnpm 9+
- 构建 macOS 应用还需要 Xcode Command Line Tools

```bash
git clone https://github.com/Heiheiaoo/heigate.git
cd heigate
pnpm --dir frontend install
make test
```

## 提交前

```bash
gofmt -w ./cmd ./internal
go vet ./...
go test ./...
pnpm --dir frontend run typecheck
pnpm --dir frontend run lint
pnpm --dir frontend run test:run
```

CI 会跑同样的检查，外加 `go test -race` 和 macOS 上的 `.app` 构建。

## 一些约定

- **改动尽量小而专注。** 一个 PR 做一件事，方便 review 也方便回滚。
- **新逻辑配测试。** `internal/probe` 和 `internal/protocol` 是纯函数为主的包，很容易测；HTTP 层用 `httptest`。
- **注释解释「为什么」，不解释「做了什么」。** 代码本身能说明后者。
- **不要引入新依赖，除非确实必要。** 当前后端只依赖 Gin 和纯 Go 的 SQLite 驱动，前端运行时只依赖 Vue。这是有意为之。
- **注意安全边界。** 网关监听本机端口，改动 `internal/middleware` 或任何认证、Host 校验、CORS 相关的代码时请格外小心，并说明理由。
- **不要提交密钥或本地数据。** `.gitignore` 已经排除了 `*.sqlite`、`gateway.key`、`.env`，请确认没有绕过。

## 提交信息

用简短的祈使句，前面加个类型前缀：

```
feat: 支持自定义探活间隔
fix: 流式响应中断时补齐 message_stop
docs: 补充 Codex 配置说明
```

## 安全问题

请不要用公开 issue 报告漏洞，按 [SECURITY.md](SECURITY.md) 私下联系。
