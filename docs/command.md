# Command System

## Overview

Custom commands extend termojinal with interactive scripts. A command is any executable (shell script, Python, Deno, compiled binary) that communicates with termojinal through **line-delimited JSON over stdin/stdout**. Each JSON object occupies exactly one line and is terminated by a newline character.

Commands appear in the command palette (Cmd+Shift+P) alongside built-in actions. When a user selects a command, termojinal spawns it as a child process, pipes its stdout to the command palette UI, and writes user responses back to its stdin. The command's stderr is forwarded to termojinal's log for diagnostics.

The environment variable `TERMOJINAL_SOCKET` is set to the path of termojinal's IPC Unix socket, allowing commands to interact with the application directly if needed.

Commands run with their working directory set to the command's own directory (the folder containing `command.toml`).

## Directory Structure

Commands are discovered from `~/.config/termojinal/commands/`. Each command lives in its own subdirectory containing a `command.toml` manifest and an executable entry point:

```
~/.config/termojinal/commands/
├── my-command/
│   ├── command.toml    # Metadata manifest
│   └── run.sh          # Executable entry point
├── another-command/
│   ├── command.toml
│   └── main.py
```

termojinal scans every immediate subdirectory of the commands directory on launch. A subdirectory is loaded as a command if and only if it contains a valid `command.toml` and the referenced run script exists. Directories without `command.toml` are silently ignored; malformed manifests are logged as warnings and skipped.

Loaded commands are sorted alphabetically by name for deterministic ordering in the palette.

## command.toml

The manifest file describes a command's metadata and entry point. It must contain a `[command]` table with the fields listed below.

### Example

```toml
[command]
name = "My Command"
description = "What this command does"
icon = "star"           # SF Symbol name
version = "1.0.0"
author = "you"
run = "./run.sh"        # Relative path to the executable
tags = ["git", "tools"]
```

### Field Reference

| Field         | Type       | Required | Default | Description                                                        |
|---------------|------------|----------|---------|--------------------------------------------------------------------|
| `name`        | string     | Yes      | --      | Human-readable name displayed in the command palette.              |
| `description` | string     | Yes      | --      | Short description shown beneath the name in the palette.           |
| `run`         | string     | Yes      | --      | Relative path from the command directory to the executable script. |
| `icon`        | string     | No       | `""`    | SF Symbol name for the command icon (e.g. `"star"`, `"trash"`).    |
| `version`     | string     | No       | `""`    | Semantic version string.                                           |
| `author`      | string     | No       | `""`    | Author name or identifier.                                        |
| `tags`        | string[]   | No       | `[]`    | Search tags for filtering in the palette.                          |
| `signature`   | string     | No       | `null`  | Hex-encoded Ed25519 signature. See [Command Signing](#command-signing). |

## JSON Protocol

The protocol consists of **messages** (command to termojinal, written to stdout) and **responses** (termojinal to command, written to stdin). Every message and response is a single JSON object on its own line, discriminated by the `"type"` field.

### Lifecycle

1. termojinal spawns the command process.
2. The command writes a JSON message to stdout.
3. If the message is interactive (`fuzzy`, `multi`, `confirm`, `text`), termojinal presents the UI and writes the user's response to the command's stdin.
4. The command reads the response from stdin, does its work, and sends the next message.
5. The loop continues until the command sends `done` or `error`, or the process exits.

Fire-and-forget messages (`info`, `done`, `error`) do not produce a response. The command should not attempt to read from stdin after sending them.

### Messages from Command to Termojinal (stdout)

#### 1. `fuzzy` -- Fuzzy Selection List

Presents a filterable list of items. The user selects exactly one.

```json
{
  "type": "fuzzy",
  "prompt": "Select a branch",
  "items": [
    {
      "value": "main",
      "label": "main",
      "description": "Default branch",
      "preview": "Last commit: fix typo in README",
      "icon": "arrow.triangle.branch"
    },
    {
      "value": "feature/login",
      "label": "feature/login",
      "description": "WIP login page"
    }
  ],
  "preview": true
}
```

| Field     | Type         | Required | Default | Description                                             |
|-----------|--------------|----------|---------|---------------------------------------------------------|
| `type`    | `"fuzzy"`    | Yes      | --      | Message type discriminator.                             |
| `prompt`  | string       | Yes      | --      | Prompt text shown above the list.                       |
| `items`   | FuzzyItem[]  | Yes      | --      | Array of selectable items (see below).                  |
| `preview` | boolean      | No       | `false` | Whether to show the preview pane for item previews.     |

**FuzzyItem fields:**

| Field         | Type   | Required | Default       | Description                                         |
|---------------|--------|----------|---------------|-----------------------------------------------------|
| `value`       | string | Yes      | --            | The value returned when this item is selected.      |
| `label`       | string | No       | same as value | Display text shown in the list.                     |
| `description` | string | No       | --            | Secondary description text below the label.         |
| `preview`     | string | No       | --            | Content shown in the preview pane (plain text).     |
| `icon`        | string | No       | --            | SF Symbol name for the item icon.                   |

**Response:**

```json
{"type": "selected", "value": "main"}
```

#### 2. `multi` -- Multi-Select List

Presents a filterable list where the user can select one or more items using checkboxes.

```json
{
  "type": "multi",
  "prompt": "Select branches to delete",
  "items": [
    {"value": "feature/old", "label": "feature/old", "description": "merged 3 days ago"},
    {"value": "fix/typo", "label": "fix/typo", "description": "merged 1 week ago"}
  ]
}
```

| Field    | Type         | Required | Description                           |
|----------|--------------|----------|---------------------------------------|
| `type`   | `"multi"`    | Yes      | Message type discriminator.           |
| `prompt` | string       | Yes      | Prompt text shown above the list.     |
| `items`  | FuzzyItem[]  | Yes      | Array of selectable items.            |

**Response:**

```json
{"type": "multi_selected", "values": ["feature/old", "fix/typo"]}
```

#### 3. `confirm` -- Yes/No Dialog

Presents a confirmation dialog with Yes and No options.

```json
{
  "type": "confirm",
  "message": "Delete selected branches and their worktrees?",
  "default": false
}
```

| Field     | Type        | Required | Default | Description                                          |
|-----------|-------------|----------|---------|------------------------------------------------------|
| `type`    | `"confirm"` | Yes      | --      | Message type discriminator.                          |
| `message` | string      | Yes      | --      | The question to display.                             |
| `default` | boolean     | No       | `false` | Which option is pre-selected (true = Yes).           |

**Response:**

```json
{"type": "confirmed", "yes": true}
```

#### 4. `text` -- Text Input

Presents a single-line text input field.

```json
{
  "type": "text",
  "label": "Repository URL",
  "placeholder": "https://github.com/user/repo.git",
  "default": "",
  "completions": ["https://github.com/", "git@github.com:"]
}
```

| Field         | Type       | Required | Default | Description                                          |
|---------------|------------|----------|---------|------------------------------------------------------|
| `type`        | `"text"`   | Yes      | --      | Message type discriminator.                          |
| `label`       | string     | Yes      | --      | Label text displayed above the input.                |
| `placeholder` | string     | No       | `""`    | Placeholder text shown when the input is empty.      |
| `default`     | string     | No       | `""`    | Initial value pre-filled in the input.               |
| `completions` | string[]   | No       | `[]`    | Completion suggestions for the input.                |

**Response:**

```json
{"type": "text_input", "value": "https://github.com/user/repo.git"}
```

#### 5. `info` -- Progress / Information Message

Displays a transient information message. Does **not** produce a response. The command should continue immediately by sending the next message.

```json
{
  "type": "info",
  "message": "Cloning repository..."
}
```

| Field     | Type      | Required | Description                |
|-----------|-----------|----------|----------------------------|
| `type`    | `"info"`  | Yes      | Message type discriminator.|
| `message` | string    | Yes      | The message to display.    |

#### 6. `done` -- Command Complete

Signals successful completion. Does **not** produce a response. The command process should exit after sending this message. If `notify` is provided, termojinal triggers a macOS notification with that text.

```json
{
  "type": "done",
  "notify": "Repository cloned successfully"
}
```

| Field    | Type     | Required | Default | Description                                           |
|----------|----------|----------|---------|-------------------------------------------------------|
| `type`   | `"done"` | Yes      | --      | Message type discriminator.                           |
| `notify` | string   | No       | `null`  | If set, triggers a macOS notification with this text. |

#### 7. `error` -- Error Message

Signals that the command encountered an error. Does **not** produce a response. The command process should exit after sending this message.

```json
{
  "type": "error",
  "message": "gh CLI is not installed"
}
```

| Field     | Type      | Required | Description                   |
|-----------|-----------|----------|-------------------------------|
| `type`    | `"error"` | Yes      | Message type discriminator.   |
| `message` | string    | Yes      | Error description to display. |

### Responses from Termojinal to Command (stdin)

Responses are written to the command's stdin as a single JSON line after the user completes an interactive message.

| Response Type    | Sent After   | Fields                  | Description                                |
|------------------|--------------|-------------------------|--------------------------------------------|
| `selected`       | `fuzzy`      | `value: string`         | The value of the selected item.            |
| `multi_selected` | `multi`      | `values: string[]`      | Array of selected item values.             |
| `confirmed`      | `confirm`    | `yes: boolean`          | Whether the user confirmed (true) or declined (false). |
| `text_input`     | `text`       | `value: string`         | The text entered by the user.              |
| `cancelled`      | any          | (none)                  | The user pressed Escape to cancel.         |

The `cancelled` response can arrive in reply to any interactive message. Commands should always handle it, typically by sending `done` and exiting.

```json
{"type": "cancelled"}
```

## Writing Commands

### Bash Example

A complete command that lists git branches, lets the user pick one, confirms the switch, and reports the result:

**command.toml:**

```toml
[command]
name = "Switch Branch"
description = "Interactively switch git branches"
icon = "arrow.triangle.branch"
run = "./run.sh"
tags = ["git"]
```

**run.sh:**

```bash
#!/usr/bin/env bash
set -euo pipefail

# Step 1: Build the item list from git branches
branches=$(git branch --format='%(refname:short)')
items="["
first=true
while IFS= read -r branch; do
    [ -z "$branch" ] && continue
    if [ "$first" = true ]; then first=false; else items+=","; fi
    items+="{\"value\":\"$branch\",\"label\":\"$branch\"}"
done <<< "$branches"
items+="]"

# Step 2: Show fuzzy selection
echo "{\"type\":\"fuzzy\",\"prompt\":\"Switch to branch\",\"items\":$items}"

# Step 3: Read the response
read -r response

# Handle cancellation
type=$(echo "$response" | jq -r '.type // empty')
if [ "$type" = "cancelled" ]; then
    echo '{"type":"done"}'
    exit 0
fi

selected=$(echo "$response" | jq -r '.value // empty')

# Step 4: Confirm
echo "{\"type\":\"confirm\",\"message\":\"Switch to $selected?\",\"default\":true}"
read -r confirm_response

confirmed=$(echo "$confirm_response" | jq -r '.yes')
if [ "$confirmed" != "true" ]; then
    echo '{"type":"done"}'
    exit 0
fi

# Step 5: Perform the action
echo "{\"type\":\"info\",\"message\":\"Switching to $selected...\"}"
git checkout "$selected" 2>/dev/null

# Step 6: Done with notification
echo "{\"type\":\"done\",\"notify\":\"Switched to $selected\"}"
```

### Python Example

A command using Python's `json` module:

**command.toml:**

```toml
[command]
name = "Create File"
description = "Create a new file from a template"
icon = "doc.badge.plus"
run = "./run.py"
tags = ["file", "template"]
```

**run.py:**

```python
#!/usr/bin/env python3
import json
import sys
import os

def send(msg):
    """Write a JSON message to stdout."""
    print(json.dumps(msg), flush=True)

def receive():
    """Read a JSON response from stdin."""
    line = sys.stdin.readline().strip()
    return json.loads(line)

# Step 1: Choose a template
send({
    "type": "fuzzy",
    "prompt": "Select template",
    "items": [
        {"value": "py", "label": "Python", "description": "Python script with main()"},
        {"value": "sh", "label": "Shell", "description": "Bash script with set -euo pipefail"},
        {"value": "rs", "label": "Rust", "description": "Rust main.rs"},
    ]
})

response = receive()
if response["type"] == "cancelled":
    send({"type": "done"})
    sys.exit(0)

template = response["value"]

# Step 2: Get the filename
send({
    "type": "text",
    "label": "Filename",
    "placeholder": f"example.{template}",
    "default": "",
})

response = receive()
if response["type"] == "cancelled":
    send({"type": "done"})
    sys.exit(0)

filename = response["value"]

# Step 3: Create the file
send({"type": "info", "message": f"Creating {filename}..."})

templates = {
    "py": '#!/usr/bin/env python3\n\ndef main():\n    pass\n\nif __name__ == "__main__":\n    main()\n',
    "sh": '#!/usr/bin/env bash\nset -euo pipefail\n\n',
    "rs": 'fn main() {\n    println!("Hello, world!");\n}\n',
}

with open(filename, "w") as f:
    f.write(templates.get(template, ""))

send({"type": "done", "notify": f"Created {filename}"})
```

### Deno / TypeScript Example

The `sdk/` directory in the termojinal repository provides a typed Deno SDK (`@termojinal/sdk`) with high-level helpers that wrap the JSON protocol. Import from `mod.ts` to access `fuzzy`, `multi`, `confirm`, `text`, `info`, `done`, and `error` functions.

**command.toml:**

```toml
[command]
name = "Run Task"
description = "Select and run a project task"
icon = "play.circle"
run = "./run.ts"
tags = ["tasks"]
```

**run.ts:**

```typescript
#!/usr/bin/env -S deno run --allow-read

import { fuzzy, confirm, info, done, CancelledError } from "@termojinal/sdk";

try {
  const selected = await fuzzy("Select a task", [
    { value: "build", label: "Build", description: "Run cargo build" },
    { value: "test", label: "Test", description: "Run cargo test" },
    { value: "lint", label: "Lint", description: "Run clippy" },
  ]);

  const ok = await confirm(`Run ${selected}?`);
  if (!ok) {
    done();
    Deno.exit(0);
  }

  info(`Running ${selected}...`);
  // ... perform the task ...
  done(`${selected} completed!`);
} catch (e) {
  if (e instanceof CancelledError) {
    done();
  } else {
    throw e;
  }
}
```

The SDK exports the following:

**High-level functions** (handle protocol serialization and cancellation):

- `fuzzy(prompt, items)` -- returns the selected value string
- `multi(prompt, items)` -- returns an array of selected value strings
- `confirm(message, default?)` -- returns a boolean
- `text(label, options?)` -- returns the entered string
- `info(message)` -- fire-and-forget progress display
- `done(notify?)` -- signal completion, optional macOS notification
- `error(message)` -- signal error and exit

**Low-level I/O** (for custom protocol handling):

- `send(message)` -- write a `CommandMessage` JSON line to stdout
- `receive()` -- read a `CommandResponse` JSON line from stdin

All interactive functions throw `CancelledError` when the user presses Escape, providing a clean pattern for cancellation handling with try/catch.

## Command Signing

Commands can be cryptographically signed using Ed25519 to establish trust. The signing status affects how commands appear in the command palette:

| Status       | Palette Display     | Description                                          |
|--------------|---------------------|------------------------------------------------------|
| Unsigned     | "Plugin"            | No signature present. Default for user-created commands. |
| Verified     | "Verified" with checkmark | Signature matches the official termojinal public key.  |
| Invalid      | Warning indicator   | Signature present but verification failed.           |

### Signing Workflow

1. **Generate a keypair** (one-time setup):

   ```sh
   termojinal-sign --generate-key
   ```

   This outputs the secret key (hex-encoded, 64 characters) and the corresponding public key. Keep the secret key safe.

2. **Sign a command:**

   ```sh
   termojinal-sign path/to/command.toml <secret-key-hex>
   ```

   This computes an Ed25519 signature over the TOML content (excluding the `signature` field itself) and writes the hex-encoded signature into the `command.toml` file as the `signature` field.

3. **Verification** happens automatically at load time. termojinal reads the `signature` field, strips it from the TOML content, and verifies the remaining content against the embedded official public key.

### Signed command.toml example

```toml
[command]
name = "My Signed Command"
description = "A verified command"
icon = "checkmark.seal"
run = "./run.sh"
signature = "a1b2c3d4...64_bytes_hex_encoded..."
```

## Bundled Commands

The following commands ship with termojinal in the `commands/` directory of the repository. They serve as reference implementations of the protocol.

### hello-world

**Protocol demonstration.** Presents a fuzzy list of greetings in different languages, shows an info message with the selection, and finishes with a macOS notification. A minimal starting point for understanding the command lifecycle.

- Icon: `hand.wave`
- Tags: `demo`, `example`

### start-review

**GitHub PR review workflow.** Queries `gh pr list` for PRs awaiting your review, presents them in a fuzzy selector, fetches the selected branch, and sets up a git worktree for isolated review. Requires the `gh` CLI to be installed and authenticated.

- Icon: `arrow.triangle.branch`
- Tags: `github`, `review`, `claude`

### switch-worktree

**Git worktree switching.** Lists existing git worktrees via `git worktree list`, presents them in a fuzzy selector showing the directory name and branch, and signals the selected path for workspace switching.

- Icon: `arrow.triangle.swap`
- Tags: `git`, `worktree`

### kill-merged

**Merged branch cleanup.** Finds branches already merged into `main`, presents them in a multi-select list (showing associated worktree paths if any), asks for confirmation before the destructive operation, then removes both the worktrees and branches. Demonstrates the full interactive loop: `multi` followed by `confirm` followed by action.

- Icon: `trash`
- Tags: `git`, `cleanup`, `worktree`

### clone-and-open

**Clone and open repository.** Prompts for a repository URL via text input, asks for the target directory (defaulting to `~/repos`), clones the repository (or offers to open it if the directory already exists), and finishes with a notification. Demonstrates chaining multiple `text` inputs with `confirm` fallback.

- Icon: `square.and.arrow.down`
- Tags: `git`, `clone`

### run-agent

**Launch AI agent.** Presents a fuzzy selector of AI agents (Claude Code, Codex CLI, Aider), asks for a working directory via text input, validates that the selected agent is installed, and signals the launch. Demonstrates `error` type for missing dependencies.

- Icon: `brain`
- Tags: `ai`, `agent`, `claude`

## Tips

- **JSON construction in shell:** Use `jq` for building JSON when available. For simple cases, string concatenation works but requires careful escaping. The bundled commands show both approaches.

- **Working directory:** Commands run with their CWD set to the command's own directory (where `command.toml` lives), not the user's project directory. Access the project directory through environment variables or by resolving paths relative to the shell's original `$PWD` if needed.

- **Cancellation is always possible:** The user can press Escape at any interactive step. Always check for `{"type":"cancelled"}` responses and exit cleanly by sending `{"type":"done"}`.

- **Preview content:** The `preview` field in fuzzy items is rendered as plain text in the preview pane. Set `"preview": true` on the `fuzzy` message to enable the preview pane.

- **Notifications:** The `notify` field on `done` messages triggers a native macOS notification via NSNotificationCenter. Use it to alert the user when a long-running command finishes.

- **Stderr for debugging:** The command's stderr is forwarded to termojinal's log. Use `echo "debug info" >&2` in shell scripts or `eprint!` / `print(..., file=sys.stderr)` in other languages for diagnostic output without interfering with the protocol.

- **One JSON object per line:** The protocol is strictly line-delimited. Do not pretty-print JSON messages. Each message must be a single line terminated by `\n`.

- **Make scripts executable:** The run script must have execute permission. On Unix, run `chmod +x run.sh` after creating it.
