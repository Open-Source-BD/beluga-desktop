import { useCallback } from "react";
import { chooseChatFolder, setChatFolder } from "../../lib/ipc";
import { toUiError } from "../chat/errors";
import { rememberFolder } from "./folder";

interface Options {
  chatId: string | null;
  /** The chat, created first when none is open yet. */
  ensureChat: () => Promise<string>;
  /** Reloads the chat's state (and so its folder). */
  refresh: (chatId: string) => Promise<void>;
  onError: (message: string) => void;
}

/** Opening, changing and closing the chat's folder. The core shows the dialog. */
export function useFolder({ chatId, ensureChat, refresh, onError }: Options) {
  /** True when a folder was chosen; false when the user cancelled or it failed. */
  const open = useCallback(async (): Promise<boolean> => {
    try {
      const id = await ensureChat();
      const path = await chooseChatFolder(id);
      if (!path) return false;
      rememberFolder(path);
      await refresh(id);
      return true;
    } catch (e) {
      onError(toUiError(e).message);
      return false;
    }
  }, [ensureChat, refresh, onError]);

  const close = useCallback(async () => {
    if (!chatId) return;
    try {
      await setChatFolder(chatId, null);
      await refresh(chatId);
    } catch (e) {
      onError(toUiError(e).message);
    }
  }, [chatId, refresh, onError]);

  return { open, close };
}
