.PHONY: all install build build-dev app app-dev run-dev run-dev-debug run-daemon run-mcp clean test fmt lint check

# Default: build in release mode
all: build

# Install all required tools and dependencies
install:
	@echo "==> Checking Rust toolchain..."
	@command -v rustc >/dev/null 2>&1 || { echo "Installing Rust..."; curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y; }
	@echo "==> Rust $$(rustc --version)"
	@if command -v rustup >/dev/null 2>&1; then \
		echo "==> Installing cargo components..."; \
		rustup component add clippy rustfmt; \
	else \
		echo "==> rustup not found (Homebrew Rust?), skipping component install"; \
	fi
	@echo "==> Fetching crate dependencies..."
	cargo fetch
	@echo "==> Done."

# Build release binaries (termojinal, termojinald, tm, termojinal-mcp, termojinal-sign)
build:
	cargo build --release --bin termojinal
	cargo build --release -p termojinal-session --bin termojinald
	cargo build --release -p termojinal-ipc --bin tm
	cargo build --release -p termojinal-mcp --bin termojinal-mcp
	cargo build --release -p termojinal-ipc --bin termojinal-sign
	@echo "==> Release binaries in target/release/"

# Build Termojinal.app (release)
app: build
	./dist/macos/build-app.sh
	@echo "==> Termojinal.app ready at target/release/Termojinal.app"

# Build Termojinal.app (debug)
app-dev: build-dev
	./dist/macos/build-app.sh --debug
	@echo "==> Termojinal.app (debug) ready at target/debug/Termojinal.app"

# Build all dev binaries (termojinal-dev, termojinald, tm)
build-dev:
	cargo build --bin termojinal-dev
	cargo build -p termojinal-session --bin termojinald
	cargo build -p termojinal-ipc --bin tm

# Run in development mode (full stack: daemon + app, mirrors release)
# The daemon runs from inside the .app bundle so Accessibility permission
# covers both binaries under a single grant to Termojinal.app.
run-dev: app-dev
	@mkdir -p /tmp/termojinal-dev-logs
	@DAEMON=target/debug/Termojinal.app/Contents/MacOS/termojinald; \
	echo "==> Starting termojinald (from .app bundle)..."; \
	RUST_LOG=info "$$DAEMON" > /tmp/termojinal-dev-logs/daemon.log 2>&1 & echo $$! > /tmp/termojinald-dev.pid; \
	sleep 0.5; \
	if grep -q "Accessibility permission not yet granted" /tmp/termojinal-dev-logs/daemon.log 2>/dev/null; then \
		echo ""; \
		echo "  [!!] Accessibility permission required for global hotkeys (Ctrl+\`)"; \
		echo "       System Settings > Privacy & Security > Accessibility"; \
		echo "       Add: Termojinal Dev (or the .app)"; \
		echo ""; \
	fi; \
	echo "==> Opening Termojinal Dev.app..."; \
	echo "    daemon log: /tmp/termojinal-dev-logs/daemon.log"; \
	open -W target/debug/Termojinal.app; \
	kill $$(cat /tmp/termojinald-dev.pid) 2>/dev/null; \
	rm -f /tmp/termojinald-dev.pid; \
	echo "==> Stopped termojinald"

# Run in development mode with debug logging (full stack)
run-dev-debug: app-dev
	@mkdir -p /tmp/termojinal-dev-logs
	@DAEMON=target/debug/Termojinal.app/Contents/MacOS/termojinald; \
	echo "==> Starting termojinald (debug)..."; \
	RUST_LOG=debug "$$DAEMON" > /tmp/termojinal-dev-logs/daemon-debug.log 2>&1 & echo $$! > /tmp/termojinald-dev.pid; \
	sleep 0.5; \
	echo "==> Starting Termojinal Dev (debug)..."; \
	echo "    daemon log: /tmp/termojinal-dev-logs/daemon-debug.log"; \
	RUST_LOG=debug target/debug/Termojinal.app/Contents/MacOS/termojinal; \
	kill $$(cat /tmp/termojinald-dev.pid) 2>/dev/null; \
	rm -f /tmp/termojinald-dev.pid; \
	echo "==> Stopped termojinald"

# Run the session daemon standalone
run-daemon:
	RUST_LOG=info cargo run -p termojinal-session --bin termojinald

# Run the MCP server (stdio transport)
run-mcp:
	RUST_LOG=info cargo run -p termojinal-mcp --bin termojinal-mcp

# Run all tests
test:
	cargo test --workspace

# Format code
fmt:
	cargo fmt --all

# Lint with clippy
lint:
	cargo clippy --workspace -- -D warnings

# Check without building
check:
	cargo check --workspace

# Clean build artifacts
clean:
	cargo clean
