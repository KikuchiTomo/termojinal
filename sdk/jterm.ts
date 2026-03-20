import { send, receive } from "./io.ts";
import type {
  FuzzyItem,
  CommandResponse,
} from "./types.ts";

/** User cancelled the operation */
export class CancelledError extends Error {
  constructor() {
    super("User cancelled");
    this.name = "CancelledError";
  }
}

function assertNotCancelled(response: CommandResponse): void {
  if (response.type === "cancelled") throw new CancelledError();
}

/**
 * Show a fuzzy search list and return the selected value.
 * @throws {CancelledError} if the user presses Escape
 */
export async function fuzzy(prompt: string, items: FuzzyItem[]): Promise<string> {
  send({ type: "fuzzy", prompt, items });
  const response = await receive();
  assertNotCancelled(response);
  if (response.type === "selected") return response.value;
  throw new Error(`Unexpected response type: ${response.type}`);
}

/**
 * Show a multi-select list and return the selected values.
 * @throws {CancelledError} if the user presses Escape
 */
export async function multi(prompt: string, items: FuzzyItem[]): Promise<string[]> {
  send({ type: "multi", prompt, items });
  const response = await receive();
  assertNotCancelled(response);
  if (response.type === "multi_selected") return response.values;
  throw new Error(`Unexpected response type: ${response.type}`);
}

/**
 * Show a confirmation dialog and return the result.
 * @throws {CancelledError} if the user presses Escape
 */
export async function confirm(message: string, defaultValue = true): Promise<boolean> {
  send({ type: "confirm", message, default: defaultValue });
  const response = await receive();
  assertNotCancelled(response);
  if (response.type === "confirmed") return response.yes;
  throw new Error(`Unexpected response type: ${response.type}`);
}

/**
 * Show a text input and return the entered value.
 * @throws {CancelledError} if the user presses Escape
 */
export async function text(
  label: string,
  options?: { placeholder?: string; default?: string; completions?: string[] },
): Promise<string> {
  send({
    type: "text",
    label,
    placeholder: options?.placeholder ?? "",
    default: options?.default ?? "",
    completions: options?.completions,
  });
  const response = await receive();
  assertNotCancelled(response);
  if (response.type === "text_input") return response.value;
  throw new Error(`Unexpected response type: ${response.type}`);
}

/** Show a progress/info message. Does not wait for response. */
export function info(message: string): void {
  send({ type: "info", message });
}

/** Signal command completion. Optionally send a macOS notification. */
export function done(notify?: string): void {
  send({ type: "done", notify });
}

/** Signal an error and exit. */
export function error(message: string): never {
  send({ type: "error", message });
  Deno.exit(1);
}
