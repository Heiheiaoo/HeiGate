# HeiGate 开发指南

本文档记录 HeiGate 桌面应用的本地开发、测试和发布流程。

## 项目结构

| 目录 | 说明 |
|------|------|
| `backend/` | Go 本地网关服务，嵌入前端资源 |
| `frontend/` | Vue 3 管理界面 |
| `desktop/macos/` | macOS Cocoa + WebKit 应用外壳 |
| `.github/workflows/release.yml` | GitHub Release 自动打包流程 |

## 本地环境

- Go 1.27+
- Node.js 20+
- pnpm 9+
- macOS 构建需要 Xcode Command Line Tools（包含 `swiftc`）

安装前端依赖：

```bash
cd frontend
pnpm install
```

## 开发和测试

```bash
# 前端开发服务器
pnpm --dir frontend run dev

# 前端检查
pnpm --dir frontend run lint:check
pnpm --dir frontend run typecheck
pnpm --dir frontend run test:run

# 后端测试
make -C backend test-unit
```

## 构建桌面应用

构建本地桌面网关二进制：

```bash
make build-desktop
DATA_DIR="$PWD/desktop-data" ./dist/desktop/heigate-desktop -desktop
```

构建可双击启动的 macOS Apple silicon 应用：

```bash
make build-desktop-macos
open dist/desktop/HeiGate.app
```

应用数据默认保存在 `~/Library/Application Support/HeiGate`，可通过
`DATA_DIR` 指定其他目录。发布包当前面向 Apple silicon（arm64），未配置
Apple Developer 签名或公证。

## 发布

1. 在 `main` 分支提交并推送代码。
2. 创建版本标签，例如 `git tag v0.1.0 && git push origin v0.1.0`。
3. GitHub Actions 会自动构建前端、Go 本地网关和 macOS 原生外壳。
4. Release 页面会上传 `.dmg`、`.zip` 和 `checksums.txt`。

发布流程不构建或推送 Docker 镜像。
