class Termojinal < Formula
  desc "GPU-accelerated terminal emulator with AI agent coordination"
  homepage "https://github.com/KikuchiTomo/termojinal"
  url "https://github.com/KikuchiTomo/termojinal.git", branch: "main"
  version "0.1.0"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "build", "--release", "--bin", "termojinal"
    system "cargo", "build", "--release", "-p", "termojinal-session", "--bin", "termojinald"
    system "cargo", "build", "--release", "-p", "termojinal-ipc", "--bin", "tm"
    system "cargo", "build", "--release", "-p", "termojinal-mcp", "--bin", "termojinal-mcp"
    system "cargo", "build", "--release", "-p", "termojinal-ipc", "--bin", "termojinal-sign"

    bin.install "target/release/termojinal"
    bin.install "target/release/termojinald"
    bin.install "target/release/tm"
    bin.install "target/release/termojinal-mcp"
    bin.install "target/release/termojinal-sign"

    # Install app icon for desktop notifications
    (pkgshare/"icon").install "resources/Assets.xcassets/AppIcon.appiconset/256.png" => "icon.png"

    # Install default config if not present
    (etc/"termojinal").mkpath
    (etc/"termojinal/commands").mkpath

    # Install bundled commands
    (pkgshare/"commands").install Dir["commands/*"]

    # Create default config symlink hint
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

    # Create ~/.config/termojinal/ if it doesn't exist
    config_dir = Pathname.new(Dir.home)/".config/termojinal"
    unless config_dir.exist?
      config_dir.mkpath
      ohai "Created #{config_dir}"
    end

    # Symlink bundled commands if user commands dir is empty
    commands_dir = config_dir/"commands"
    unless commands_dir.exist?
      commands_dir.mkpath
      Dir[pkgshare/"commands/*"].each do |cmd_dir|
        target = commands_dir/File.basename(cmd_dir)
        ln_s cmd_dir, target unless target.exist?
      end
      ohai "Linked bundled commands to #{commands_dir}"
    end
  end

  def caveats
    <<~EOS
      To start the termojinal daemon (enables global hotkeys like Ctrl+`):
        brew services start termojinal

      To configure termojinal:
        mkdir -p ~/.config/termojinal
        cp #{opt_pkgshare}/config.example.toml ~/.config/termojinal/config.toml

      Bundled commands are installed at:
        #{opt_pkgshare}/commands/

      Note: termojinald requires Accessibility permission for global hotkeys.
      Go to System Settings > Privacy & Security > Accessibility
      and add termojinald (or your terminal app).
    EOS
  end

  test do
    assert_match "termojinal", shell_output("#{bin}/termojinal --version 2>&1", 1)
  end
end
