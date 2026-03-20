# @jterm/sdk

TypeScript/Deno SDK for writing jterm command scripts.

jterm commands are external scripts that communicate with jterm via line-delimited JSON over stdin/stdout. This SDK provides typed helpers for the protocol so you can focus on your command logic.

## Usage

```typescript
#!/usr/bin/env -S deno run --allow-read

import { fuzzy, confirm, info, done, CancelledError } from "@jterm/sdk";

try {
  const selected = await fuzzy("Select a task", [
    { value: "build", label: "Build", description: "Run cargo build" },
    { value: "test", label: "Test", description: "Run cargo test" },
  ]);

  const ok = await confirm(`Run ${selected}?`);
  if (!ok) { done(); Deno.exit(0); }

  info(`Running ${selected}...`);
  // ... do work ...
  done(`${selected} completed!`);
} catch (e) {
  if (e instanceof CancelledError) done();
  else throw e;
}
```

## API

### High-level functions

- **`fuzzy(prompt, items)`** - Show a fuzzy search list, returns the selected value
- **`multi(prompt, items)`** - Show a multi-select list, returns selected values
- **`confirm(message, default?)`** - Show a yes/no dialog, returns boolean
- **`text(label, options?)`** - Show a text input, returns the entered string
- **`info(message)`** - Show a progress/info message (fire-and-forget)
- **`done(notify?)`** - Signal completion, optionally with a macOS notification
- **`error(message)`** - Signal an error and exit

### Low-level I/O

- **`send(message)`** - Write a `CommandMessage` JSON line to stdout
- **`receive()`** - Read a `CommandResponse` JSON line from stdin

### Types

All protocol types are exported: `CommandMessage`, `CommandResponse`, `FuzzyItem`, and all individual message/response interfaces.

## Development

```sh
deno task check   # Type-check
deno task test    # Run tests
```
