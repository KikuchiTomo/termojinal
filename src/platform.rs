//! Platform-specific process detection utilities.

use std::collections::HashMap;

pub(crate) fn get_child_cwd(pid: i32) -> Option<String> {
    let output = std::process::Command::new("lsof")
        .args(["-a", "-d", "cwd", "-p", &pid.to_string(), "-Fn"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if let Some(path) = line.strip_prefix('n') {
            if path.starts_with('/') {
                return Some(path.to_string());
            }
        }
    }
    None
}

/// Detect the foreground child process name of a PTY shell.
/// Returns the process name if a non-shell child is running (e.g. "python3",
/// "cargo", "node"), or None if only the shell itself is running.
pub(crate) fn detect_foreground_child(pty_pid: i32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["ax", "-o", "ppid=,pid=,command="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);

    // Build child map: ppid -> [(pid, command)]
    let mut children: HashMap<i32, Vec<(i32, String)>> = HashMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(3, char::is_whitespace);
        let ppid: i32 = parts
            .next()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(-1);
        let _child_pid: i32 = parts
            .next()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(-1);
        let cmd = parts.next().unwrap_or("").trim().to_string();
        if ppid >= 0 && _child_pid >= 0 {
            children.entry(ppid).or_default().push((_child_pid, cmd));
        }
    }

    // Look for direct children of the PTY process.
    let kids = children.get(&pty_pid)?;
    // Filter out shell processes (the login shell is always there).
    let shells = ["-zsh", "zsh", "-bash", "bash", "-fish", "fish", "-sh", "sh"];
    for (_, cmd) in kids {
        let basename = cmd.split_whitespace().next().unwrap_or("");
        let basename = std::path::Path::new(basename)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(basename);
        if !shells.contains(&basename) && !basename.is_empty() {
            return Some(basename.to_string());
        }
    }
    None
}

/// Detect SSH connection from a PTY's child process tree.
/// Walks the process tree starting from `pid` to find an `ssh` child process,
/// then parses its arguments to extract user@host.
pub(crate) fn detect_ssh_from_pid(pid: i32) -> Option<(Option<String>, String)> {
    // Get all processes with ppid,pid,command to build child lookup.
    let output = std::process::Command::new("ps")
        .args(["ax", "-o", "ppid=,pid=,command="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);

    // Build a map: ppid -> [(pid, command)]
    let mut children: HashMap<i32, Vec<(i32, String)>> = HashMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(3, char::is_whitespace);
        let ppid: i32 = parts
            .next()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(-1);
        let child_pid: i32 = parts
            .next()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(-1);
        let cmd = parts.next().unwrap_or("").trim().to_string();
        if ppid >= 0 && child_pid >= 0 {
            children.entry(ppid).or_default().push((child_pid, cmd));
        }
    }

    // BFS from pid to find ssh in descendants.
    let mut queue = vec![pid];
    let mut visited = std::collections::HashSet::new();
    while let Some(current) = queue.pop() {
        if !visited.insert(current) {
            continue;
        }
        if let Some(kids) = children.get(&current) {
            for (child_pid, cmd) in kids {
                queue.push(*child_pid);
                if let Some(result) = parse_ssh_command(cmd) {
                    return Some(result);
                }
            }
        }
    }
    None
}

/// Parse an ssh command string to extract (Option<user>, host).
/// Uses `ssh -G <destination>` to resolve the actual hostname and user
/// from ~/.ssh/config, supporting aliases like `ssh mydev`.
pub(crate) fn parse_ssh_command(cmd: &str) -> Option<(Option<String>, String)> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    let bin = std::path::Path::new(parts[0])
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if bin != "ssh" {
        return None;
    }

    // Find destination: first non-flag argument.
    let mut i = 1;
    let mut destination = None;
    while i < parts.len() {
        let arg = parts[i];
        if arg.starts_with('-') {
            if matches!(
                arg,
                "-p" | "-i"
                    | "-l"
                    | "-o"
                    | "-F"
                    | "-J"
                    | "-L"
                    | "-R"
                    | "-D"
                    | "-W"
                    | "-b"
                    | "-c"
                    | "-e"
                    | "-m"
                    | "-S"
                    | "-w"
            ) {
                i += 1;
            }
        } else {
            destination = Some(arg.to_string());
            break;
        }
        i += 1;
    }

    let dest = destination?;

    // Use `ssh -G <dest>` to resolve actual user and hostname from ssh config.
    // Output is `key value` pairs, one per line. We split on first whitespace.
    if let Ok(output) = std::process::Command::new("ssh")
        .args(["-G", &dest])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut resolved_user = None;
            let mut resolved_host = None;
            for line in text.lines() {
                let line = line.trim();
                if let Some(idx) = line.find(char::is_whitespace) {
                    let key = &line[..idx];
                    let val = line[idx..].trim();
                    match key.to_lowercase().as_str() {
                        "user" => resolved_user = Some(val.to_string()),
                        "hostname" => resolved_host = Some(val.to_string()),
                        _ => {}
                    }
                }
            }
            if let Some(host) = resolved_host {
                return Some((resolved_user, host));
            }
        }
    }

    // Fallback: parse destination directly.
    if let Some(at_idx) = dest.find('@') {
        Some((
            Some(dest[..at_idx].to_string()),
            dest[at_idx + 1..].to_string(),
        ))
    } else {
        Some((None, dest))
    }
}
