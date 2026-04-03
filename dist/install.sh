#!/usr/bin/env bash
set -euo pipefail

# termojinal installer — for users who don't use Homebrew
# Usage: curl -fsSL https://raw.githubusercontent.com/KikuchiTomo/termojinal/main/dist/install.sh | bash

echo "==> Installing termojinal..."

# Check Rust
if ! command -v cargo &>/dev/null; then
    echo "==> Installing Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
fi

# Clone and build
TERMOJINAL_DIR="${TERMOJINAL_DIR:-$HOME/.local/share/termojinal-install}"
if [ -d "$TERMOJINAL_DIR" ]; then
    echo "==> Updating termojinal..."
    cd "$TERMOJINAL_DIR" && git pull
else
    echo "==> Cloning termojinal..."
    git clone https://github.com/KikuchiTomo/termojinal.git "$TERMOJINAL_DIR"
    cd "$TERMOJINAL_DIR"
fi

echo "==> Building (release)..."
cargo build --release --bin termojinal
cargo build --release -p termojinal-session --bin termojinald

# Install binaries
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "$INSTALL_DIR"
cp target/release/termojinal "$INSTALL_DIR/termojinal"
cp target/release/termojinald "$INSTALL_DIR/termojinald"
cp target/release/tm "$INSTALL_DIR/tm"

echo "==> Installed to $INSTALL_DIR/"

# Setup config
CONFIG_DIR="$HOME/.config/termojinal"
mkdir -p "$CONFIG_DIR/commands"

# Link bundled commands
for cmd_dir in commands/*/; do
    cmd_name=$(basename "$cmd_dir")
    target="$CONFIG_DIR/commands/$cmd_name"
    if [ ! -e "$target" ]; then
        ln -s "$(pwd)/$cmd_dir" "$target"
    fi
done

# Install launchd plist for daemon
PLIST_SRC="dist/launchd/com.termojinal.daemon.plist"
PLIST_DST="$HOME/Library/LaunchAgents/com.termojinal.daemon.plist"

if [ ! -f "$PLIST_DST" ]; then
    # Update path to actual binary location
    sed "s|/usr/local/bin/termojinald|$INSTALL_DIR/termojinald|g" "$PLIST_SRC" | \
    sed "s|/usr/local/var/log|$HOME/.local/var/log|g" > "$PLIST_DST"
    mkdir -p "$HOME/.local/var/log/termojinal"
    echo "==> Installed launchd plist"
fi

echo ""
echo "==> termojinal installed successfully!"
echo ""
echo "  Start the daemon:    launchctl load $PLIST_DST"
echo "  Run termojinal:           termojinal"
echo "  Quick Terminal:      Ctrl+\` (requires daemon + Accessibility permission)"
echo ""
echo "  Config:  $CONFIG_DIR/config.toml"
echo "  Daemon:  launchctl start com.termojinal.daemon"
echo ""

# Check PATH
if ! echo "$PATH" | grep -q "$INSTALL_DIR"; then
    echo "  NOTE: Add $INSTALL_DIR to your PATH:"
    echo "    export PATH=\"$INSTALL_DIR:\$PATH\""
    echo ""
fi
