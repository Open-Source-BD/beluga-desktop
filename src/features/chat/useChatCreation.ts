import { useCallback, useState } from "react";
import { flushSync } from "react-dom";
import { createChat, type UiError } from "../../lib/ipc";
import { inheritLastFolder } from "../mentions/folder";
import { toUiError } from "./errors";

interface Options {
  selectedId: string | null;
  modelId: string | null;
  setSelectedId: (id: string) => void;
  refreshChats: () => void;
}

/**
 * Creates the chat on demand (the first message, or opening a folder before
 * any message). A new chat inherits the last folder used. Rejects when the
 * chat cannot be created, leaving `startError` for the empty screen to show.
 */
export function useChatCreation({ selectedId, modelId, setSelectedId, refreshChats }: Options) {
  const [creating, setCreating] = useState(false);
  const [startError, setStartError] = useState<UiError | null>(null);

  const ensureChat = useCallback(async (): Promise<string> => {
    if (selectedId) return selectedId;
    if (!modelId) throw new Error("no model");
    setCreating(true);
    setStartError(null);
    try {
      const id = (await createChat(modelId)).id;
      await inheritLastFolder(id);
      flushSync(() => setSelectedId(id));
      refreshChats();
      return id;
    } catch (e) {
      setStartError(toUiError(e));
      throw e;
    } finally {
      setCreating(false);
    }
  }, [selectedId, modelId, setSelectedId, refreshChats]);

  return { creating, startError, setStartError, ensureChat };
}
