.PHONY: help frontend backend desktop app app-macos-arm64 windows package-windows test test-backend test-frontend lint fmt fmt-check check install-frontend clean run

VERSION ?= $(shell tr -d '\r\n' < VERSION)
LDFLAGS := -s -w -X main.version=$(VERSION)
APP_DIR := dist/HeiGate.app

help:
	@echo "HeiGate build targets:"
	@echo "  make frontend   - build the web UI into internal/web/dist"
	@echo "  make backend    - build the local gateway binary for this machine (dist/heigate)"
	@echo "  make desktop    - build the gateway binary for macOS arm64 with the UI embedded"
	@echo "  make app        - build the macOS arm64 HeiGate.app (alias of app-macos-arm64)"
	@echo "  make windows    - build the Windows x64 desktop app (run on Windows with PowerShell 7)"
	@echo "  make package-windows - build the Windows x64 zip and installer (NSIS required)"
	@echo "  make run        - run the gateway from source with a local data directory"
	@echo "  make test       - run backend and frontend tests"
	@echo "  make lint       - run go vet and frontend lint/typecheck"
	@echo "  make fmt        - gofmt-format the Go sources in place"
	@echo "  make check      - run the full pre-commit gate (fmt-check, vet, race tests, UI, build)"

frontend:
	@pnpm --dir frontend install --frozen-lockfile
	@pnpm --dir frontend run build

# Install frontend dependencies when they are missing or older than the manifest
# and lockfile. Tracking node_modules as a file target lets make skip the
# registry round-trip when nothing changed, while still reinstalling when
# dependencies actually move — so `make check` cannot quietly run against stale
# node_modules and diverge from CI's fresh install.
frontend/node_modules: frontend/package.json frontend/pnpm-lock.yaml
	@pnpm --dir frontend install --frozen-lockfile
	@touch frontend/node_modules

install-frontend: frontend/node_modules

backend:
	@mkdir -p dist
	@CGO_ENABLED=0 go build -tags embed -trimpath -ldflags="$(LDFLAGS)" -o dist/heigate ./cmd/heigate

desktop: frontend
	@mkdir -p dist
	@CGO_ENABLED=0 GOOS=darwin GOARCH=arm64 go build -tags embed -trimpath -ldflags="$(LDFLAGS)" -o dist/heigate-darwin-arm64 ./cmd/heigate

app: app-macos-arm64

app-macos-arm64: frontend
	@command -v swiftc >/dev/null 2>&1 || (echo "swiftc not found: install Xcode Command Line Tools" >&2; exit 1)
	@rm -rf $(APP_DIR)
	@mkdir -p $(APP_DIR)/Contents/MacOS $(APP_DIR)/Contents/Resources
	@CGO_ENABLED=0 GOOS=darwin GOARCH=arm64 go build -tags embed -trimpath -ldflags="$(LDFLAGS)" -o $(APP_DIR)/Contents/MacOS/heigate-gateway ./cmd/heigate
	@sed "s/__VERSION__/$(VERSION)/g" desktop/macos/Info.plist > $(APP_DIR)/Contents/Info.plist
	@cp desktop/macos/AppIcon.icns $(APP_DIR)/Contents/Resources/AppIcon.icns
	@swiftc -target arm64-apple-macos12.0 -O -framework Cocoa -framework WebKit desktop/macos/HeiGateApp.swift -o $(APP_DIR)/Contents/MacOS/HeiGate
	@chmod +x $(APP_DIR)/Contents/MacOS/HeiGate $(APP_DIR)/Contents/MacOS/heigate-gateway
	@codesign --force --deep --sign - $(APP_DIR)
	@codesign --verify --deep --strict --verbose=2 $(APP_DIR)
	@echo "Built $(APP_DIR)"

windows:
	@pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/build-windows.ps1 -Version "$(VERSION)"

package-windows:
	@pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/package-windows.ps1 -Version "$(VERSION)"

run:
	@mkdir -p desktop-data
	@DATA_DIR="$(CURDIR)/desktop-data" go run ./cmd/heigate

test: test-backend test-frontend

test-backend:
	@go test -race -count=1 ./...

test-frontend: install-frontend
	@pnpm --dir frontend run typecheck
	@pnpm --dir frontend run test:run

lint: install-frontend
	@go vet ./...
	@pnpm --dir frontend run lint

fmt:
	@gofmt -w ./cmd ./internal

# Fails (listing the offenders) when any Go source is not gofmt-clean, matching
# the CI gate so `make check` reproduces it locally.
fmt-check:
	@unformatted=$$(gofmt -l ./cmd ./internal); \
	if [ -n "$$unformatted" ]; then \
		echo "These files need gofmt:"; echo "$$unformatted"; exit 1; \
	fi

# Mirror of the CI pipeline so a green `make check` means a green CI. Built from
# the individual targets rather than re-listing their commands, so a flag change
# in one of them cannot silently leave this out of step with CI.
check: fmt-check lint test-backend test-frontend
	@pnpm --dir frontend run build

clean:
	@rm -rf dist frontend/dist internal/web/dist/assets internal/web/dist/index.html internal/web/dist/*.svg internal/web/dist/*.png
