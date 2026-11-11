import { approvedFolders, setChatFolder } from "../../lib/ipc";

const KEY = "lastFolder";

/** The path as shown: Windows stores folders with a leading `\\?\`. */
export function displayPath(path: string): string {
  return path.startsWith("\\\\?\\") ? path.slice(4) : path;
}

/** The folder's last path component, for either separator. */
export function folderName(path: string): string {
  const parts = displayPath(path).split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] ?? displayPath(path);
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Remembers the folder as a hint for new chats; never trusted without `approvedFolders`. */
export function rememberFolder(path: string): void {
  try {
    localStorage.setItem(KEY, path);
  } catch {
    // A hint only; ignore storage failures.
  }
}

/** Gives a new chat the last folder used, if it is still the most recently approved one. */
export async function inheritLastFolder(chatId: string): Promise<void> {
  try {
    const last = localStorage.getItem(KEY);
    if (!last) return;
    const [first] = await approvedFolders();
    if (first === last) await setChatFolder(chatId, last);
  } catch {
    // The folder may be gone; the chat simply starts without one.
  }
}
