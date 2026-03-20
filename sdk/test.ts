import { assertEquals } from "https://deno.land/std/testing/asserts.ts";
import type { FuzzyItem, CommandMessage, CommandResponse } from "./types.ts";

Deno.test("FuzzyItem type check", () => {
  const item: FuzzyItem = {
    value: "test",
    label: "Test Item",
    description: "A test",
  };
  assertEquals(item.value, "test");
});

Deno.test("CommandMessage serialization", () => {
  const msg: CommandMessage = {
    type: "fuzzy",
    prompt: "Select",
    items: [{ value: "a", label: "A" }],
  };
  const json = JSON.stringify(msg);
  const parsed = JSON.parse(json);
  assertEquals(parsed.type, "fuzzy");
  assertEquals(parsed.items.length, 1);
});

Deno.test("CommandResponse deserialization", () => {
  const json = '{"type":"selected","value":"test"}';
  const response: CommandResponse = JSON.parse(json);
  assertEquals(response.type, "selected");
  if (response.type === "selected") {
    assertEquals(response.value, "test");
  }
});

Deno.test("DoneMessage with notify", () => {
  const msg: CommandMessage = { type: "done", notify: "Task complete" };
  const json = JSON.stringify(msg);
  const parsed = JSON.parse(json);
  assertEquals(parsed.notify, "Task complete");
});

Deno.test("ErrorMessage", () => {
  const msg: CommandMessage = { type: "error", message: "Something broke" };
  assertEquals(msg.type, "error");
  assertEquals(msg.message, "Something broke");
});
