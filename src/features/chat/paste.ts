import type { AttachmentInput } from "../../lib/ipc";

/** Pasted text at least this long becomes an attachment instead of filling the box. */
export const PASTE_AS_FILE_CHARS = 4000;

const NAME = /^Pasted text(?: \d+)?\.txt$/;

/**
 * The text as a "Pasted text.txt" attachment. `taken` lists the names already
 * in the message, so a second one is "Pasted text 2.txt".
 */
export function pastedTextAttachment(text: string, taken: string[] = []): AttachmentInput {
  let name = "Pasted text.txt";
  for (let n = 2; taken.includes(name); n++) name = `Pasted text ${n}.txt`;
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return { name, mime: "text/plain", data_base64: btoa(binary) };
}

/** The text of a pasted-text attachment, or null for any other attachment. */
export function pastedTextOf(a: AttachmentInput): string | null {
  if (!NAME.test(a.name)) return null;
  try {
    const binary = atob(a.data_base64);
    return new TextDecoder().decode(Uint8Array.from(binary, (c) => c.charCodeAt(0)));
  } catch {
    return null;
  }
}
