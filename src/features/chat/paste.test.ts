import { describe, expect, it } from "vitest";
import { PASTE_AS_FILE_CHARS, pastedTextAttachment, pastedTextOf } from "./paste";

describe("paste", () => {
  it("turns text into a 'Pasted text.txt' attachment and reads it back", () => {
    expect(PASTE_AS_FILE_CHARS).toBe(4000);
    const a = pastedTextAttachment("héllo\nworld");
    expect(a.name).toBe("Pasted text.txt");
    expect(a.mime).toBe("text/plain");
    expect(pastedTextOf(a)).toBe("héllo\nworld");
  });

  it("numbers a second one in the same message", () => {
    expect(pastedTextAttachment("x", ["Pasted text.txt"]).name).toBe("Pasted text 2.txt");
    expect(pastedTextAttachment("x", ["Pasted text.txt", "Pasted text 2.txt"]).name).toBe("Pasted text 3.txt");
  });

  it("does not read other attachments as pasted text", () => {
    expect(pastedTextOf({ name: "a.txt", mime: "text/plain", data_base64: "eA==" })).toBeNull();
  });
});
