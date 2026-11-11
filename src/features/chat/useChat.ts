import { useCallback, useEffect, useRef, useState } from "react";
import {
  compactChat,
  getMessages,
  retryTurn,
  sendMessage,
  stop as stopTurn,
  type AttachmentInput,
  type EngineEvent,
  type StoredMessage,
  type TurnResult,
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
  /** The send was rejected before the core stored anything: `pendingUser` was not sent. */
  unsent: { attachments: AttachmentInput[] } | null;
}

const EMPTY: ChatState = {
  messages: [],
  streamingText: "",
  pendingUser: null,
  status: "idle",
  error: null,
  unsent: null,
};

/** How a /compact ended: the core's result, or the rejection when it never ran. */
export interface CompactOutcome {
  result?: TurnResult;
  error?: UiError;
}

export interface UseChat extends ChatState {
  /** Sends to `targetChatId` (default: the selected chat). Ignored while that chat is streaming. */
  send(
    text: string,
    attachments: AttachmentInput[],
    modelId: string,
    targetChatId?: string,
  ): void;
  stop(): void;
  /**
   * Shortens the selected chat. It runs like a reply (status "streaming", Stop
   * cancels it); `onOutcome` hears how it ended, so a failure is reported by
   * the caller and does not become a thread error with a Retry.
   */
  compact(modelId: string, onOutcome: (outcome: CompactOutcome) => void, keep?: string): void;
  /**
   * Answers again. A message the core rejected is sent anew; otherwise the
   * stored last user message is answered without being stored twice.
   * `modelId` overrides the model of the previous send.
   */
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

  /** Runs one turn; `pending` is the just-typed text not yet in the stored list. */
  const start = useCallback(
    (
      id: string,
      modelId: string,
      pending: { text: string; attachments: AttachmentInput[] } | null,
      run: (onEvent: (e: EngineEvent) => void) => Promise<TurnResult>,
    ) => {
      if (streaming.current.has(id)) return;
      streaming.current.add(id);
      lastModel.current[id] = modelId;
      patch(id, () => ({
        streamingText: "",
        pendingUser: pending?.text ?? null,
        status: "streaming",
        error: null,
        unsent: null,
      }));

      // The command's promise is the only end-of-turn signal: it settles after
      // the core has stored the reply. Channel events only carry reply text.
      let settled = false;
      const settle = async (
        status: ChatStatus,
        error: UiError | null,
        notSent = false,
      ) => {
        settled = true;
        const messages = await reload(id);
        streaming.current.delete(id);
        patch(id, (s) => ({
          messages: messages ?? s.messages,
          streamingText: "",
          // A rejected send stored nothing: keep the text so it can be retried.
          pendingUser: notSent || !messages ? s.pendingUser : null,
          unsent: notSent && pending ? { attachments: pending.attachments } : null,
          status,
          error,
        }));
        onTurnEndRef.current?.(id);
      };
      const onEvent = (event: EngineEvent) => {
        if (settled || event.type !== "text_delta") return;
        patch(id, (s) => ({ streamingText: s.streamingText + event.text }));
      };
      run(onEvent).then(
        (r) => {
          if (r.status === "failed") {
            return settle("failed", r.error ?? toUiError(null));
          }
          return settle(r.status === "interrupted" ? "interrupted" : "idle", null);
        },
        (e: unknown) => settle("failed", toUiError(e), true),
      );
    },
    [patch, reload],
  );

  const send = useCallback<UseChat["send"]>(
    (text, attachments, modelId, targetChatId) => {
      const id = targetChatId ?? chatId;
      if (!id) return;
      start(id, modelId, { text, attachments }, (onEvent) =>
        sendMessage(id, text, attachments, modelId, onEvent),
      );
    },
    [chatId, start],
  );

  const stop = useCallback(() => {
    if (chatId && streaming.current.has(chatId)) {
      stopTurn(chatId).catch(() => {});
    }
  }, [chatId]);

  const retry = useCallback(
    (modelId?: string | null) => {
      if (!chatId) return;
      const model = modelId ?? lastModel.current[chatId];
      if (!model) return;
      const { pendingUser, unsent } = currentRef.current;
      if (unsent && pendingUser !== null) {
        // Never stored: send it again as a new message.
        send(pendingUser, unsent.attachments, model);
      } else {
        const id = chatId;
        start(id, model, null, (onEvent) => retryTurn(id, model, onEvent));
      }
    },
    [chatId, send, start],
  );

  const compact = useCallback<UseChat["compact"]>(
    (modelId, onOutcome, keep) => {
      if (!chatId) return;
      const id = chatId;
      start(id, modelId, null, async () => {
        try {
          const result = keep ? await compactChat(id, modelId, keep) : await compactChat(id, modelId);
          onOutcome({ result });
          if (result.status !== "failed") return result;
        } catch (e) {
          onOutcome({ error: toUiError(e) });
        }
        return { status: "done", reason: null, error: null };
      });
    },
    [chatId, start],
  );

  return { ...current, send, stop, retry, compact };
}
