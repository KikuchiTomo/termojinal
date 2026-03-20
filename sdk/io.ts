import type { CommandMessage, CommandResponse } from "./types.ts";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** Send a message to jterm (write JSON line to stdout) */
export function send(message: CommandMessage): void {
  const line = JSON.stringify(message) + "\n";
  Deno.stdout.writeSync(encoder.encode(line));
}

/** Read a response from jterm (read JSON line from stdin) */
export async function receive(): Promise<CommandResponse> {
  // Read one line from stdin
  const buf = new Uint8Array(4096);
  let data = "";
  while (true) {
    const n = await Deno.stdin.read(buf);
    if (n === null) throw new Error("stdin closed");
    data += decoder.decode(buf.subarray(0, n));
    const newline = data.indexOf("\n");
    if (newline >= 0) {
      const line = data.substring(0, newline).trim();
      return JSON.parse(line) as CommandResponse;
    }
  }
}
