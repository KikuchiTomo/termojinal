class Termojinal < Formula
  desc "GPU-accelerated terminal emulator with AI agent coordination (CLI tools)"
  homepage "https://github.com/KikuchiTomo/termojinal"
  TERMOJINAL_VERSION = "0.2.5-beta"
  version TERMOJINAL_VERSION
  license "MIT"

  # Pre-built universal binaries from GitHub Releases (built by CI)
  url "https://github.com/KikuchiTomo/termojinal/releases/download/v#{TERMOJINAL_VERSION}/termojinal-#{TERMOJINAL_VERSION}-cli-macos-universal.tar.gz"
  sha256 "ee436d2ff1ba4c09fe822f1ce6aa5dcd31c984906a4cc745da84377171c27be6"

  def install
    # Install pre-built CLI binaries
    bin.install "termojinal"
    bin.install "termojinald"
    bin.install "tm"
    bin.install "termojinal-mcp"
    bin.install "termojinal-sign"

    # Install default config example
    (pkgshare/"config.example.toml").write default_config
  end

  def default_config
    <<~TOML
      # termojinal configuration
      # Copy to ~/.config/termojinal/config.toml and customize

      [font]
      family = "monospace"
      size = 14.0
      line_height = 1.2

      [window]
      opacity = 0.95

      [startup]
      mode = "fixed"
      directory = "~"

      [quick_terminal]
      enabled = true
      hotkey = "ctrl+`"
      animation = "slide_down"
      height_ratio = 0.4
    TOML
  end

  # launchd plist for termojinald daemon
  service do
    run [opt_bin/"termojinald"]
    keep_alive true
    log_path var/"log/termojinal/termojinald.log"
    error_log_path var/"log/termojinal/termojinald.err.log"
    environment_variables RUST_LOG: "info"
    working_dir HOMEBREW_PREFIX
  end

  def post_install
    (var/"log/termojinal").mkpath

    # Create config directory
    config_dir = Pathname.new(Dir.home)/".config/termojinal"
    config_dir.mkpath unless config_dir.exist?
  end

  def caveats
    <<~EOS
      CLI tools installed: termojinal, termojinald, tm, termojinal-mcp, termojinal-sign

      To install the GUI app (Termojinal.app):
        brew install --cask termojinal

      To start the daemon (enables Ctrl+` global hotkey):
        brew services start termojinal

      Run `tm setup` to configure Claude Code hooks and bundled commands.

      To configure:
        cp #{opt_pkgshare}/config.example.toml ~/.config/termojinal/config.toml
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/tm --version 2>&1", 0)
  end
end
