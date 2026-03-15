//! Quick integration test: spawn a PTY, check we get output.

use jterm_pty::{Pty, PtyConfig, PtySize};

#[test]
fn pty_produces_output() {
    let config = PtyConfig {
        shell: "/bin/zsh".to_string(),
        size: PtySize { cols: 80, rows: 24 },
        ..PtyConfig::default()
    };
    let pty = Pty::spawn(&config).expect("spawn");

    // Give the shell time to start and print a prompt.
    std::thread::sleep(std::time::Duration::from_millis(500));

    let mut buf = [0u8; 4096];
    let n = pty.read(&mut buf).expect("read");
    assert!(n > 0, "expected PTY output, got 0 bytes");

    let output = String::from_utf8_lossy(&buf[..n]);
    eprintln!("PTY output ({n} bytes): {:?}", &output[..output.len().min(200)]);
}

#[test]
fn pty_echo_input() {
    let config = PtyConfig {
        shell: "/bin/zsh".to_string(),
        size: PtySize { cols: 80, rows: 24 },
        ..PtyConfig::default()
    };
    let pty = Pty::spawn(&config).expect("spawn");
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Drain initial output.
    let mut buf = [0u8; 65536];
    let _ = pty.read(&mut buf);

    // Send a command.
    pty.write(b"echo hello_jterm\r").expect("write");
    std::thread::sleep(std::time::Duration::from_millis(500));

    let n = pty.read(&mut buf).expect("read");
    let output = String::from_utf8_lossy(&buf[..n]);
    eprintln!("after echo ({n} bytes): {:?}", &output[..output.len().min(500)]);
    assert!(
        output.contains("hello_jterm"),
        "expected 'hello_jterm' in output"
    );
}
