.PHONY: build build-backend build-frontend build-desktop build-desktop-macos test test-backend test-frontend test-frontend-critical

FRONTEND_CRITICAL_VITEST := \
	src/api/__tests__/client.spec.ts \
	src/api/__tests__/tokenRefresh.spec.ts \
	src/api/__tests__/channelMonitorV2.spec.ts \
	src/views/auth/__tests__/LinuxDoCallbackView.spec.ts \
	src/views/auth/__tests__/WechatCallbackView.spec.ts \
	src/views/user/__tests__/PaymentView.spec.ts \
	src/views/user/__tests__/PaymentResultView.spec.ts \
	src/views/user/__tests__/ChannelStatusView.mode.spec.ts \
	src/components/user/profile/__tests__/ProfileInfoCard.spec.ts \
	src/views/admin/__tests__/SettingsView.spec.ts \
	src/features/channel-monitor-v2/__tests__/designSystem.structure.spec.ts \
	src/features/channel-monitor-v2/__tests__/monitorFormat.spec.ts \
	src/features/channel-monitor-v2/__tests__/monitorZoom.spec.ts

# 一键编译前后端
build: build-backend build-frontend

# 编译后端（复用 backend/Makefile）
build-backend:
	@$(MAKE) -C backend build

# 编译前端（需要已安装依赖）
build-frontend:
	@pnpm --dir frontend run build

# Build a self-contained local-only binary. The embedded frontend is generated
# first; runtime data is kept outside the repository through DATA_DIR.
build-desktop: build-frontend
	@mkdir -p dist/desktop
	@command -v go >/dev/null 2>&1 || (echo "需要安装 Go 1.27+ 才能编译桌面服务" >&2; exit 1)
	@cd backend && CGO_ENABLED=0 go build -tags embed -trimpath -ldflags="-s -w" -o ../dist/desktop/heigate-desktop ./cmd/server

# Build a double-clickable macOS ARM64 application bundle.
build-desktop-macos: build-frontend
	@mkdir -p dist/desktop/HeiGate.app/Contents/MacOS dist/desktop/HeiGate.app/Contents/Resources
	@command -v go >/dev/null 2>&1 || (echo "需要安装 Go 1.27+ 才能编译 macOS 桌面应用" >&2; exit 1)
	@command -v swiftc >/dev/null 2>&1 || (echo "需要安装 Xcode Command Line Tools 才能编译 macOS 桌面应用" >&2; exit 1)
	@cd backend && CGO_ENABLED=0 GOOS=darwin GOARCH=arm64 go build -tags embed -trimpath -ldflags="-s -w" -o "../dist/desktop/HeiGate.app/Contents/MacOS/heigate-desktop-bin" ./cmd/server
	@VERSION="$${VERSION:-$$(tr -d '\r\n' < backend/cmd/server/VERSION)}"; sed "s/<string>0\.1\.0<\/string>/<string>$${VERSION}<\/string>/g" desktop/macos/Info.plist > dist/desktop/HeiGate.app/Contents/Info.plist
	@cp desktop/macos/AppIcon.icns dist/desktop/HeiGate.app/Contents/Resources/AppIcon.icns
	@swiftc -target arm64-apple-macos12.0 -O -framework Cocoa -framework WebKit desktop/macos/HeiGateDesktop.swift -o dist/desktop/HeiGate.app/Contents/MacOS/HeiGate
	@chmod +x dist/desktop/HeiGate.app/Contents/MacOS/HeiGate dist/desktop/HeiGate.app/Contents/MacOS/heigate-desktop-bin

# 运行测试（后端 + 前端）
test: test-backend test-frontend

test-backend:
	@$(MAKE) -C backend test

test-frontend:
	@pnpm --dir frontend run lint:check
	@pnpm --dir frontend run typecheck
	@$(MAKE) test-frontend-critical

test-frontend-critical:
	@pnpm --dir frontend exec vitest run $(FRONTEND_CRITICAL_VITEST)
