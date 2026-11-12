import { useCallback, useEffect, useRef, useState } from "react";
import {
  chatUsage,
  compactChat,
  getMessages,
  retryTurn,
  sendMessage,
  stop as stopTurn,
  type AttachmentInput,
  type ChatUsage,
  type EngineEvent,
  type PlanItem,
  type StoredMessage,
  type TurnResult,
  type UiError,
} from "../../lib/ipc";
import { answeredLive, OUTPUT_KEEP, pairStored, type TrailEntry } from "../agent/trailEntries";
import { toUiError } from "./errors";

/** "paused": a project task stopped and waits for the user; the chat is usable and nothing failed. */
export type ChatStatus = "idle" | "streaming" | "failed" | "interrupted" | "paused";

export interface ChatState {
  messages: StoredMessage[];
  streamingText: string;
  /** The text just sent, shown until the core's stored copy is reloaded. */
  pendingUser: string | null;
  status: ChatStatus;
  error: UiError | null;
  /** The send was rejected before the core stored anything: `pendingUser` was not sent. */
  unsent: { attachments: AttachmentInput[] } | null;
  /** Why the task paused (a `PauseReason`), while `status` is "paused". */
  pausedReason: string | null;
  /** Tool calls of the running task, from events; stored ones leave when their result is stored. */
  live: TrailEntry[];
  /** Seconds of a rate-limit wait being served. */
  waiting: number | null;
  /** The checklist as the running task last sent it; null until it sends one. */
  plan: PlanItem[] | null;
  /** The task's running request and cost totals, and the chat's totals when it began. */
  taskUsage: ChatUsage | null;
  usageBase: ChatUsage | null;
  /** Rows stored during the task (each `message_stored`). */
  storedCount: number;
  /** Changes when the checklist, background commands or approvals may have changed. */
  taskTick: number;
}

let liveSeq = 0;

const EMPTY: ChatState = {
  messages: [],
  streamingText: "",
  pendingUser: null,
  status: "idle",
  error: null,
  unsent: null,
  pausedReason: null,
  live: [],
  waiting: null,
  plan: null,
  taskUsage: null,
  usageBase: null,
  storedCount: 0,
  taskTick: 0,
};

/** How a /compact ended: the core's result, or the rejection when it never ran. */
export interface CompactOutcome {
  result?: TurnResult;
  error?: UiError;
}

export interface UseChat extends ChatState {
  /** The chat's request count and cost including the running task; null when unknown. */
  usage: ChatUsage | null;
  /**
   * Sends to `targetChatId` (default: the selected chat). Ignored while that chat is
   * streaming; the result says whether it was accepted.
   */
  send(
    text: string,
    attachments: AttachmentInput[],
    modelId: string,
    targetChatId?: string,
  ): boolean;
  stop(): void;
  /** Loads the stored messages again (after the core removed some) and clears a stale failure. */
  reload(): Promise<void>;
  /** Drops everything held for a deleted chat; later events and results for it are ignored. */
  forget(chatId: string): void;
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
  /**
   * Answers the last user message again, replacing the stored reply (a new
   * request). `modelId` overrides the model of the previous send.
   */
  regenerate(modelId?: string | null): void;
}

const lastIndexOf = (live: TrailEntry[], callId: string) => {
  for (let i = live.length - 1; i >= 0; i--) if (live[i].callId === callId) return i;
  return -1;
};

/** Line ends and trailing space the core may change when it stores a reply. */
const norm = (t: string) => t.replace(/\r\n/g, "\n");

/**
 * The state after rows were stored mid-task. A stored reply with text is the truth for the
 * reply in progress: its text leaves the streaming text (anything streamed after it stays,
 * unless that is only whitespace) and, when it cannot be found there, the streamed text is
 * dropped rather than shown twice. A stored user row replaces the just-typed copy, and a live
 * line leaves when the tool row of its own call (matched by position) is stored.
 */
function withStoredRows(s: ChatState, messages: StoredMessage[]): Partial<ChatState> {
  const known = new Set(s.messages.map((m) => m.id));
  const fresh = messages.filter((m) => !known.has(m.id));
  let text = s.streamingText;
  for (const m of fresh) {
    if (m.kind !== "message" || m.role !== "assistant" || !m.text.trim()) continue;
    const stored = norm(m.text).trimEnd();
    const lead = norm(text).trimStart();
    text = lead.startsWith(stored) ? lead.slice(stored.length) : "";
  }
  if (!text.trim()) text = "";
  const answered = answeredLive(
    pairStored(messages),
    s.live,
    new Set(fresh.filter((m) => m.kind === "tool").map((m) => m.id)),
  );
  return {
    messages,
    streamingText: text,
    pendingUser: fresh.some((m) => m.kind === "message" && m.role === "user") ? null : s.pendingUser,
    live: s.live.filter((e) => !answered.has(e)),
  };
}

/**
 * State is kept per chat, so a reply that is still streaming when the user
 * switches chats keeps updating its own thread and never leaks into the
 * visible one. The core stores the messages; this hook reloads them when a
 * turn ends instead of building them client-side.
 */
export function useChat(
  chatId: string | null,
  onTurnEnd?: (chatId: string, status: ChatStatus) => void,
): UseChat {
  const [states, setStates] = useState<Record<string, ChatState>>({});
  const streaming = useRef(new Set<string>());
  const loadSeq = useRef<Record<string, number>>({});
  const lastModel = useRef<Record<string, string>>({});
  const turnSeq = useRef<Record<string, number>>({});
  const deleted = useRef(new Set<string>());
  const onTurnEndRef = useRef(onTurnEnd);
  onTurnEndRef.current = onTurnEnd;
  const current = (chatId && states[chatId]) || EMPTY;
  const currentRef = useRef(current);
  currentRef.current = current;

  const patch = useCallback(
    (id: string, change: (s: ChatState) => Partial<ChatState>) =>
      setStates((all) => {
        if (deleted.current.has(id)) return all;
        const s = all[id] ?? EMPTY;
        return { ...all, [id]: { ...s, ...change(s) } };
      }),
    [],
  );

  const reload = useCallback(async (id: string): Promise<StoredMessage[] | null> => {
    if (deleted.current.has(id)) return null;
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
    ): boolean => {
      if (streaming.current.has(id)) return false;
      streaming.current.add(id);
      lastModel.current[id] = modelId;
      const turn = (turnSeq.current[id] ?? 0) + 1;
      turnSeq.current[id] = turn;
      patch(id, () => ({
        streamingText: "",
        pendingUser: pending?.text ?? null,
        status: "streaming",
        error: null,
        unsent: null,
        pausedReason: null,
        live: [],
        waiting: null,
        plan: null,
        taskUsage: null,
        usageBase: null,
      }));
      // The task's usage events carry its own totals: remember the chat's as they stand now.
      try {
        chatUsage(id).then(
          (u) => {
            if (u && turnSeq.current[id] === turn) patch(id, () => ({ usageBase: u }));
          },
          () => {},
        );
      } catch {
        // No base: the header keeps showing the stored totals.
      }

      // The command's promise is the only end-of-turn signal: it settles after
      // the core has stored the reply. Channel events only carry reply text.
      let settled = false;
      const settle = async (
        status: ChatStatus,
        error: UiError | null,
        notSent = false,
        pausedReason: string | null = null,
      ) => {
        settled = true;
        const messages = await reload(id);
        streaming.current.delete(id);
        patch(id, (s) => ({
          messages: messages ?? s.messages,
          streamingText: "",
          pausedReason,
          waiting: null,
          // The stored rows now hold every call; if they could not be read, keep the live lines.
          live: messages ? [] : s.live,
          taskTick: s.taskTick + 1,
          // A rejected send stored nothing: keep the text so it can be retried.
          pendingUser: notSent || !messages ? s.pendingUser : null,
          unsent: notSent && pending ? { attachments: pending.attachments } : null,
          status,
          error,
        }));
        if (!deleted.current.has(id)) onTurnEndRef.current?.(id, status);
      };
      const onEvent = (event: EngineEvent) => {
        if (settled) return;
        switch (event.type) {
          case "text_delta":
            patch(id, (s) => ({ streamingText: s.streamingText + event.text, waiting: null }));
            break;
          case "usage":
            patch(id, () => ({
              taskUsage: { requests: event.requests, cost_usd: event.cost_usd },
              waiting: null,
            }));
            break;
          case "tool_started":
            patch(id, (s) => ({
              waiting: null,
              live: [
                ...s.live,
                {
                  callId: event.call_id,
                  name: event.name,
                  summary: event.summary,
                  status: "running",
                  liveId: ++liveSeq,
                },
              ],
            }));
            break;
          case "tool_output":
            patch(id, (s) => {
              const at = lastIndexOf(s.live, event.call_id);
              return {
                live: s.live.map((e, i) =>
                  i === at ? { ...e, output: ((e.output ?? "") + event.text).slice(-OUTPUT_KEEP) } : e,
                ),
              };
            });
            break;
          case "tool_finished":
            patch(id, (s) => {
              const done = {
                status: event.status,
                summary: event.summary,
                detail: event.detail,
              } as const;
              const at = lastIndexOf(s.live, event.call_id);
              return {
                live:
                  at >= 0
                    ? s.live.map((e, i) =>
                        // The detail replaces what streamed in.
                        i === at ? { ...e, ...done, output: undefined } : e,
                      )
                    : [...s.live, { callId: event.call_id, name: "", liveId: ++liveSeq, ...done }],
              };
            });
            break;
          case "plan_updated":
            patch(id, (s) => ({ plan: event.items, taskTick: s.taskTick + 1 }));
            break;
          case "background_changed":
          case "approval_cleared":
            patch(id, (s) => ({ taskTick: s.taskTick + 1 }));
            break;
          case "waiting":
            patch(id, () => ({ waiting: event.seconds }));
            break;
          case "message_stored":
            patch(id, (s) => ({ storedCount: s.storedCount + 1 }));
            void reload(id).then((messages) => {
              if (messages && !settled) patch(id, (s) => withStoredRows(s, messages));
            });
            break;
          default:
            break;
        }
      };
      run(onEvent).then(
        (r) => {
          if (r.status === "failed") {
            return settle("failed", r.error ?? toUiError(null));
          }
          if (r.status === "paused") return settle("paused", null, false, r.reason);
          return settle(r.status === "interrupted" ? "interrupted" : "idle", null);
        },
        (e: unknown) => settle("failed", toUiError(e), true),
      );
      return true;
    },
    [patch, reload],
  );

  const send = useCallback<UseChat["send"]>(
    (text, attachments, modelId, targetChatId) => {
      const id = targetChatId ?? chatId;
      if (!id) return false;
      return start(id, modelId, { text, attachments }, (onEvent) =>
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

  const forget = useCallback((id: string) => {
    deleted.current.add(id);
    streaming.current.delete(id);
    delete loadSeq.current[id];
    delete lastModel.current[id];
    delete turnSeq.current[id];
    setStates(({ [id]: _gone, ...rest }) => rest);
  }, []);

  const reloadSelected = useCallback(async () => {
    if (!chatId || streaming.current.has(chatId)) return;
    const messages = await reload(chatId);
    if (messages) patch(chatId, () => ({ messages, status: "idle", error: null, unsent: null, pendingUser: null }));
  }, [chatId, reload, patch]);

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

  const regenerate = useCallback<UseChat["regenerate"]>(
    (modelId) => {
      if (!chatId) return;
      const model = modelId ?? lastModel.current[chatId];
      if (!model) return;
      const id = chatId;
      start(id, model, null, (onEvent) => retryTurn(id, model, onEvent, true));
    },
    [chatId, start],
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

  const usage =
    current.usageBase && current.taskUsage
      ? {
          requests: current.usageBase.requests + current.taskUsage.requests,
          cost_usd: current.usageBase.cost_usd + current.taskUsage.cost_usd,
        }
      : null;

  return { ...current, usage, send, stop, reload: reloadSelected, forget, retry, regenerate, compact };
}
