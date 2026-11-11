import { describe, expect, it } from "vitest";
import {
  MAX_ATTACHMENTS,
  MAX_IMAGE_BYTES,
  MAX_TEXT_BYTES,
  classifyFile,
  readAttachment,
} from "./attachments";

const file = (name: string, content: BlobPart, type = "") =>
  new File([content], name, { type });

describe("classifyFile", () => {
  it("treats code and prose files as text by extension", async () => {
    for (const name of ["a.ts", "README.md", "data.json"]) {
      expect(await classifyFile(file(name, "x"))).toBe("text");
    }
  });

  it("treats an extensionless UTF-8 file as text by sniffing", async () => {
    expect(await classifyFile(file("Makefile", "all:\n\techo hi\n"))).toBe("text");
    expect(await classifyFile(file("LICENSE", "héllo wörld"))).toBe("text");
  });

  it("treats png and jpg as images", async () => {
    expect(await classifyFile(file("a.png", new Uint8Array([137, 80]), "image/png"))).toBe("image");
    expect(await classifyFile(file("b.jpg", new Uint8Array([255, 216])))).toBe("image");
  });

  it("refuses archives and binary blobs", async () => {
    expect(await classifyFile(file("a.zip", "PK", "application/zip"))).toBe("unsupported");
    expect(await classifyFile(file("blob", new Uint8Array([1, 0, 2, 0xff])))).toBe("unsupported");
    expect(await classifyFile(file("blob.dat", new Uint8Array([0x61, 0x00, 0x62])))).toBe("unsupported");
  });
});

describe("readAttachment", () => {
  it("exposes the limits", () => {
    expect([MAX_ATTACHMENTS, MAX_TEXT_BYTES, MAX_IMAGE_BYTES]).toEqual([5, 200_000, 5_000_000]);
  });

  it("returns name, mime and base64 contents", async () => {
    const r = await readAttachment(file("n.md", "hi", "text/markdown"));
    expect(r).toEqual({ attachment: { name: "n.md", mime: "text/markdown", data_base64: "aGk=" } });
  });

  it("gives an image a mime from its extension when the browser gives none", async () => {
    const r = await readAttachment(file("p.png", new Uint8Array([1, 2, 3])));
    expect(r).toMatchObject({ attachment: { mime: "image/png" } });
  });

  it("refuses a 300 KB text file with the exact copy", async () => {
    const r = await readAttachment(file("big.txt", "a".repeat(300_000)));
    expect(r).toEqual({ error: "big.txt is too large to attach." });
  });

  it("refuses an unsupported file with the exact copy", async () => {
    const r = await readAttachment(file("a.zip", "PK"));
    expect(r).toEqual({ error: "a.zip is not a text file or image." });
  });
});
