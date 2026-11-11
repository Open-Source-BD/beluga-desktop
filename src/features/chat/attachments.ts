import type { AttachmentInput } from "../../lib/ipc";

export const MAX_ATTACHMENTS = 5;
export const MAX_TEXT_BYTES = 200_000;
export const MAX_IMAGE_BYTES = 5_000_000;

export type FileKind = "text" | "image" | "unsupported";

export const MAX_ATTACHMENTS_MESSAGE = `You can attach up to ${MAX_ATTACHMENTS} files.`;
export const CANNOT_READ_IMAGES =
  "This model cannot read images. Pick a model that can, or remove the image.";

const SNIFF_BYTES = 8192;

const TEXT_EXTENSIONS = new Set(
  (
    "txt md markdown json jsonc yaml yml toml ini cfg conf env csv tsv log xml html htm css scss less " +
    "js jsx mjs cjs ts tsx py rb rs go java kt swift c h cc cpp hpp cs php sh bash zsh fish ps1 bat " +
    "sql graphql proto lua r dart vue svelte tex rst diff patch gitignore dockerfile"
  ).split(" "),
);

const IMAGE_TYPES: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
};

const BINARY_EXTENSIONS = new Set(
  "zip gz tgz tar 7z rar bz2 xz exe dll so dylib bin dmg iso pdf doc docx xls xlsx ppt pptx mp3 mp4 mov wav avi mkv woff woff2 ttf otf class jar wasm".split(
    " ",
  ),
);

function extensionOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

function readAs<T extends string | ArrayBuffer>(read: (r: FileReader) => void): Promise<T> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(r.result as T);
    r.onerror = () => reject(r.error);
    read(r);
  });
}

/** Valid UTF-8 without NUL bytes. */
async function looksLikeText(file: File): Promise<boolean> {
  const head = file.slice(0, SNIFF_BYTES);
  const bytes = new Uint8Array(await readAs<ArrayBuffer>((r) => r.readAsArrayBuffer(head)));
  if (bytes.includes(0)) return false;
  try {
    // A slice can cut a multi-byte character in half; only that tail may be invalid.
    new TextDecoder("utf-8", { fatal: true }).decode(bytes, { stream: file.size > SNIFF_BYTES });
    return true;
  } catch {
    return false;
  }
}

/** By extension and mime first; an unknown or extensionless file is sniffed. */
export async function classifyFile(file: File): Promise<FileKind> {
  const ext = extensionOf(file.name);
  if (TEXT_EXTENSIONS.has(ext) || file.type.startsWith("text/")) return "text";
  if (ext in IMAGE_TYPES || file.type.startsWith("image/")) return "image";
  if (BINARY_EXTENSIONS.has(ext)) return "unsupported";
  return (await looksLikeText(file)) ? "text" : "unsupported";
}

export type ReadResult = { attachment: AttachmentInput } | { error: string };

/** Classifies, size-checks and base64-encodes one file. */
export async function readAttachment(file: File): Promise<ReadResult> {
  const kind = await classifyFile(file);
  if (kind === "unsupported") {
    return { error: `${file.name} is not a text file or image.` };
  }
  const limit = kind === "image" ? MAX_IMAGE_BYTES : MAX_TEXT_BYTES;
  if (file.size > limit) return { error: `${file.name} is too large to attach.` };
  const url = await readAs<string>((r) => r.readAsDataURL(file));
  const data_base64 = url.slice(url.indexOf(",") + 1);
  const mime =
    kind === "image"
      ? file.type.startsWith("image/")
        ? file.type
        : (IMAGE_TYPES[extensionOf(file.name)] ?? "image/png")
      : file.type || "text/plain";
  return { attachment: { name: file.name, mime, data_base64 } };
}
