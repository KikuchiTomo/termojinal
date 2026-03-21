class Termojinal < Formula
  desc "GPU-accelerated terminal emulator with AI agent coordination"
  homepage "https://github.com/KikuchiTomo/termojinal"
  version "0.3.0-beta"
  license "MIT"

  # Pre-built universal binaries from GitHub Releases (built by CI)
  url "https://github.com/KikuchiTomo/termojinal/releases/download/v#{version}/termojinal-#{version}-cli-macos-universal.tar.gz"
  sha256 "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

  # The .app bundle is a separate download
  resource "app" do
    url "https://github.com/KikuchiTomo/termojinal/releases/download/v#{version}/termojinal-#{version}-macos-universal.tar.gz"
    sha256 "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
  end

  def install
    # Install pre-built CLI binaries
    bin.install "termojinal"
    bin.install "termojinald"
    bin.install "tm"
    bin.install "termojinal-mcp"
    bin.install "termojinal-sign"

    # Extract and install the .app bundle
    resource("app").stage do
      prefix.install "Termojinal.app"
    end

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

    # Symlink Termojinal.app into /Applications
    app_target = Pathname.new("/Applications/Termojinal.app")
    app_source = prefix/"Termojinal.app"
    if app_source.exist? && !app_target.exist?
      ln_s app_source, app_target
      ohai "Linked Termojinal.app to /Applications"
    end

    # Create config directory
    config_dir = Pathname.new(Dir.home)/".config/termojinal"
    unless config_dir.exist?
      config_dir.mkpath
      ohai "Created #{config_dir}"
    end
  end

  def caveats
    <<~EOS
      Run `tm setup` to configure Claude Code hooks and bundled commands.

      To start the daemon (enables Ctrl+` global hotkey):
        brew services start termojinal

      To configure:
        cp #{opt_pkgshare}/config.example.toml ~/.config/termojinal/config.toml

      Termojinal.app has been linked to /Applications.
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/tm --version 2>&1", 0)
  end
end
