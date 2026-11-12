import { useCallback, useEffect, useRef, useState } from "react";
import { onBackgroundChanged, taskState, type TaskState } from "../../lib/ipc";

const NONE: TaskState = { plan: [], edits_allowed: false, background: [], project: false, waiting: null };

/**
 * The selected chat's task state (checklist, edits switch, background commands, folder,
 * waiting card). Refreshed when the chat or `tick` changes and when this chat's background
 * commands change; `refresh` asks again. An answer for a chat that is no longer selected,
 * or one overtaken by a later request, is ignored, and a chat never shows another's state.
 */
export function useTask(chatId: string | null, tick = 0) {
  const [held, setHeld] = useState<{ id: string; state: TaskState } | null>(null);
  const chatRef = useRef(chatId);
  chatRef.current = chatId;
  const seq = useRef(0);
  const latest = useRef(new Map<string, number>());

  const refresh = useCallback(async (id: string | null = chatRef.current) => {
    if (!id) return;
    const mine = ++seq.current;
    latest.current.set(id, mine);
    try {
      const s = await taskState(id);
      if (!s || latest.current.get(id) !== mine || chatRef.current !== id) return;
      setHeld({ id, state: s });
    } catch {
      // Keep what is shown.
    }
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
  };
}
