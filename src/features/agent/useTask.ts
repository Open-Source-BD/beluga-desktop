import { useCallback, useEffect, useRef, useState } from "react";
import { onBackgroundChanged, taskState, type TaskState } from "../../lib/ipc";

const NONE: TaskState = { plan: [], edits_allowed: false, background: [], project: false, waiting: null, running: false };

/**
 * The selected chat's task state (checklist, edits switch, background commands, folder,
 * waiting card). Refreshed when the chat or `tick` changes and when this chat's background
 * commands change; `refresh` asks again. An answer for a chat that is no longer selected,
 * or one overtaken by a later request, is ignored, and a chat never shows another's state.
 */
export function useTask(chatId: string | null, tick = 0) {
  const [held, setHeld] = useState<{ id: string; state: TaskState; tick: number } | null>(null);
  const tickRef = useRef(tick);
  tickRef.current = tick;
  const chatRef = useRef(chatId);
  chatRef.current = chatId;
  const seq = useRef(0);
  const latest = useRef(new Map<string, number>());
  const deleted = useRef(new Set<string>());

  const refresh = useCallback(async (id: string | null = chatRef.current) => {
    if (!id || deleted.current.has(id)) return;
    const started = tickRef.current;
    const mine = ++seq.current;
    latest.current.set(id, mine);
    try {
      const s = await taskState(id);
      if (!s || deleted.current.has(id) || latest.current.get(id) !== mine || chatRef.current !== id) return;
      setHeld({ id, state: s, tick: started });
    } catch {
      // Keep what is shown.
    }
  }, []);

  /** Drops a deleted chat's state; nothing is asked or applied for it again. */
  const forget = useCallback((id: string) => {
    deleted.current.add(id);
    latest.current.delete(id);
    setHeld((h) => (h && h.id === id ? null : h));
  }, []);

  useEffect(() => {
    if (chatId) void refresh(chatId);
  }, [chatId, tick, refresh]);

  useEffect(() => {
    let live = true;
    let unlisten: (() => void) | undefined;
    try {
      onBackgroundChanged((id) => {
        if (id === chatRef.current) void refresh(id);
      }).then(
        (u) => {
          if (live) unlisten = u;
          else u();
        },
        () => {},
      );
    } catch {
      // No event source: the checklist still refreshes with the task.
    }
    return () => {
      live = false;
      unlisten?.();
    };
  }, [refresh]);

  const s = held && held.id === chatId ? held.state : NONE;
  return {
    plan: s.plan,
    editsAllowed: s.edits_allowed,
    background: s.background,
    project: s.project,
    waiting: s.waiting,
    refresh,
    forget,
    /** The `tick` that was current when the shown state was asked for; -1 before any answer. */
    loadedTick: held && held.id === chatId ? held.tick : -1,
  };
}
