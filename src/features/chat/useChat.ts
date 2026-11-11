import { useCallback, useEffect, useRef, useState } from "react";
import {
  getMessages,
  sendMessage,
  stop as stopTurn,
  type AttachmentInput,
  type EngineEvent,
  type StoredMessage,
  type UiError,
} from "../../lib/ipc";
import { toUiError } from "./errors";

export type ChatStatus = "idle" | "streaming" | "failed" | "interrupted";

export interface ChatState {
  messages: StoredMessage[];
  streamingText: string;
  /** The text just sent, shown until the core's stored copy is reloaded. */
  pendingUser: string | null;
  status: ChatStatus;
  error: UiError | null;
}

const EMPTY: ChatState = {
  messages: [],
  streamingText: "",
  pendingUser: null,
  status: "idle",
  error: null,
};

export interface UseChat extends ChatState {
  /** Sends to `targetChatId` (default: the selected chat). Ignored while that chat is streaming. */
  send(
    text: string,
    attachments: AttachmentInput[],
    modelId: string,
    targetChatId?: string,
  ): void;
  stop(): void;
  /** Resends the last user message. `modelId` overrides the model of the previous send. */
  retry(modelId?: string | null): void;
}

/**
 * State is kept per chat, so a reply that is still streaming when the user
 * switches chats keeps updating its own thread and never leaks into the
 * visible one. The core stores the messages; this hook reloads them when a
 * turn ends instead of building them client-side.
 */
export function useChat(
  chatId: string | null,
  onTurnEnd?: (chatId: string) => void,
): UseChat {
  const [states, setStates] = useState<Record<string, ChatState>>({});
  const streaming = useRef(new Set<string>());
  const loadSeq = useRef<Record<string, number>>({});
  const lastModel = useRef<Record<string, string>>({});
  const onTurnEndRef = useRef(onTurnEnd);
  onTurnEndRef.current = onTurnEnd;
  const current = (chatId && states[chatId]) || EMPTY;
  const currentRef = useRef(current);
  currentRef.current = current;

  const patch = useCallback(
    (id: string, change: (s: ChatState) => Partial<ChatState>) =>
      setStates((all) => {
        const s = all[id] ?? EMPTY;
        return { ...all, [id]: { ...s, ...change(s) } };
      }),
    [],
  );

  const reload = useCallback(async (id: string): Promise<StoredMessage[] | null> => {
    const seq = (loadSeq.current[id] ?? 0) + 1;
    loadSeq.current[id] = seq;
    try {
      const messages = await getMessages(id);
      return loadSeq.current[id] === seq ? messages : null;
    } catch {
      return null;
    }
  }, []);

  // Selecting a chat loads it, unless a reply for it is still streaming.
  useEffect(() => {
    if (!chatId || streaming.current.has(chatId)) return;
    let live = true;
    reload(chatId).then((messages) => {
      if (!live || !messages || streaming.current.has(chatId)) return;
      patch(chatId, () => ({ messages, status: "idle", error: null }));
    });
    return () => {
      live = false;
    };
  }, [chatId, reload, patch]);

  const send = useCallback<UseChat["send"]>(
    (text, attachments, modelId, targetChatId) => {
      const id = targetChatId ?? chatId;
      if (!id || streaming.current.has(id)) return;
      streaming.current.add(id);
      lastModel.current[id] = modelId;
      patch(id, () => ({
        streamingText: "",
        pendingUser: text,
        status: "streaming",
        error: null,
      }));

      let settled = false;
      const settle = async (status: ChatStatus, error: UiError | null) => {
        if (settled) return;
        settled = true;
        const messages = await reload(id);
        streaming.current.delete(id);
        patch(id, (s) => ({
          messages: messages ?? s.messages,
          streamingText: "",
          pendingUser: messages ? null : s.pendingUser,
          status,
          error,
        }));
        onTurnEndRef.current?.(id);
      };
      const onEvent = (event: EngineEvent) => {
        if (settled) return;
        switch (event.type) {
          case "text_delta":
            patch(id, (s) => ({ streamingText: s.streamingText + event.text }));
            break;
          case "done":
            void settle("idle", null);
            break;
          case "interrupted":
            void settle("interrupted", null);
            break;
          case "failed":
            void settle("failed", event.error);
            break;
        }
      };
      sendMessage(id, text, attachments, modelId, onEvent).then(
        () => settle("idle", null),
        (e: unknown) => settle("failed", toUiError(e)),
      );
    },
    [chatId, patch, reload],
  );

  const stop = useCallback(() => {
    if (chatId && streaming.current.has(chatId)) {
      stopTurn(chatId).catch(() => {});
    }
  }, [chatId]);

  const retry = useCallback(
    (modelId?: string | null) => {
      if (!chatId) return;
      const lastUser = [...currentRef.current.messages]
        .reverse()
        .find((m) => m.role === "user");
      const model = modelId ?? lastModel.current[chatId];
      if (!lastUser || !model) return;
      send(lastUser.text, lastUser.attachments, model);
    },
    [chatId, send],
  );

  return { ...current, send, stop, retry };
}
