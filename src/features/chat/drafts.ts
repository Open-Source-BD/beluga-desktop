// Unsent text per chat, kept in localStorage so a draft survives switching chats and restarts.
// Every access is guarded: storage can be blocked, and a draft is only a convenience.

const key = (chatId: string | null) => `draft:${chatId ?? "new"}`;

/** The saved draft for a chat (`null` is the new-chat box), or "". */
export function loadDraft(chatId: string | null): string {
  try {
    return localStorage.getItem(key(chatId)) ?? "";
  } catch {
    return "";
  }
}

// Chats that were deleted: a late save (a pending timer flushed as the box unmounts) must not bring a draft back.
const discarded = new Set<string>();

export function saveDraft(chatId: string | null, text: string): void {
  if (chatId !== null && discarded.has(chatId)) return;
  try {
    localStorage.setItem(key(chatId), text);
  } catch {
    // Not remembering a draft is harmless.
  }
}

export function clearDraft(chatId: string | null): void {
  try {
    localStorage.removeItem(key(chatId));
  } catch {
    // Nothing to do.
  }
}

/** Removes a deleted chat's draft and refuses any later save for it. */
export function discardDraft(chatId: string): void {
  discarded.add(chatId);
  clearDraft(chatId);
}
