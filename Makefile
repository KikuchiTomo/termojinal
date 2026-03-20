.PHONY: all install build build-dev run-dev run-dev-debug run-daemon run-mcp clean test fmt lint check

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

# Build dev binary only (faster, unoptimized)
build-dev:
	cargo build --bin termojinal-dev

# Run in development mode (debug build, direct PTY)
run-dev:
	RUST_LOG=info cargo run --bin termojinal-dev

# Run in development mode with debug logging
run-dev-debug:
	RUST_LOG=debug cargo run --bin termojinal-dev

# Run the session daemon
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
