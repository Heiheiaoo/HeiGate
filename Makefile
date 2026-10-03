.PHONY: help install-frontend frontend run app app-macos restart restart-macos bundle test test-backend test-frontend lint fmt fmt-check check clean

CARGO ?= cargo
PNPM ?= pnpm

help:
	@echo "HeiGate build targets:"
	@echo "  make frontend   - build the web UI into frontend/dist"
	@echo "  make run        - run the desktop app from source (cargo run)"
	@echo "  make app        - build app bundles for the current platform (Tauri)"
	@echo "  make restart    - rebuild and restart on macOS only"
	@echo "  make app-macos  - build the macOS HeiGate.app bundle (Tauri)"
	@echo "  make bundle     - build every bundle target for this platform"
	@echo "  make test       - run backend and frontend tests"
	@echo "  make lint       - run cargo clippy and frontend lint"
	@echo "  make fmt        - rustfmt the Rust sources in place"
	@echo "  make check      - full pre-commit gate (fmt-check, clippy, tests, UI build)"

# Install dependencies when they are missing or older than the manifest and
# lockfile, so `make check` cannot quietly run against stale node_modules.
frontend/node_modules: frontend/package.json frontend/pnpm-lock.yaml
	@$(PNPM) --dir frontend install --frozen-lockfile
	@touch frontend/node_modules

node_modules: package.json pnpm-lock.yaml
	@$(PNPM) install --frozen-lockfile
	@touch node_modules

install-frontend: frontend/node_modules node_modules

frontend: install-frontend
	@$(PNPM) --dir frontend run build

run:
	@$(CARGO) run -p heigate-desktop

app: frontend
	@$(PNPM) exec tauri build
	@echo "Built app bundles for the current platform."

restart: app
	@if [ "$$(uname -s 2>/dev/null)" != "Darwin" ]; then echo "make restart is only supported on macOS; use make app or make run."; exit 1; fi
	@-pkill -f 'heigate-desktop' 2>/dev/null || true
	@sleep 0.5
	@open target/release/bundle/macos/HeiGate.app
	@echo "HeiGate.app has been packaged and restarted."

app-macos: frontend
	@$(PNPM) exec tauri build --bundles app
	@echo "Built target/release/bundle/macos/HeiGate.app"

restart-macos: app-macos
	@-pkill -f 'heigate-desktop' 2>/dev/null || true
	@sleep 0.5
	@open target/release/bundle/macos/HeiGate.app
	@echo "HeiGate.app has been packaged and restarted."

bundle: frontend
	@$(PNPM) exec tauri build

test: test-backend test-frontend

test-backend:
	@$(CARGO) test --workspace

test-frontend: install-frontend
	@$(PNPM) --dir frontend run typecheck
	@$(PNPM) --dir frontend run test:run

lint: install-frontend
	@$(CARGO) clippy --workspace --all-targets -- -D warnings
	@$(PNPM) --dir frontend run lint

fmt:
	@$(CARGO) fmt

# Fails when any Rust source is not rustfmt-clean, matching the CI gate.
fmt-check:
	@$(CARGO) fmt --check

# Mirror of the CI pipeline so a green `make check` means a green CI.
check: fmt-check lint test-backend test-frontend
	@$(PNPM) --dir frontend run build

clean:
	@$(CARGO) clean
	@rm -rf frontend/dist
