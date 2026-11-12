import { useCallback, useEffect, useRef, useState } from "react";
import { taskState } from "../../lib/ipc";

export const WATCH_MS = 5000;
/** The startup sweep asks this many chats at a time, and only the most recent ones. */
const BATCH = 5;
const SWEEP_MAX = 50;

interface Seen {
  running: boolean;
  waiting: boolean;
}

/**
 * What the sidebar's chats are doing that this window did not start or has lost track of: one
 * sweep when the chats first load (so a card waiting in a chat marks it), then a poll every
 * five seconds for the chats found running only. Nothing is asked for a deleted chat; every
 * timer ends with the component.
 */
export function useChatWatch(chats: { id: string; updated_at: number }[]) {
  const [seen, setSeen] = useState<Record<string, Seen>>({});
  const swept = useRef(false);
  const gone = useRef(new Set<string>());
  const seenRef = useRef(seen);
  seenRef.current = seen;

  const ask = useCallback(async (ids: string[]) => {
    const found: Record<string, Seen> = {};
    for (let i = 0; i < ids.length; i += BATCH) {
      await Promise.all(
        ids.slice(i, i + BATCH).map(async (id) => {
          try {
            const s = await taskState(id);
            if (s && !gone.current.has(id))
              found[id] = { running: s.running === true, waiting: s.waiting !== null && s.waiting !== undefined };
          } catch {
            // Nothing known for this chat; it is not marked.
          }
        }),
      );
    }
    setSeen((cur) => ({ ...cur, ...found }));
  }, []);

  useEffect(() => {
    if (swept.current || chats.length === 0) return;
    swept.current = true;
    const newest = [...chats].sort((a, b) => b.updated_at - a.updated_at).slice(0, SWEEP_MAX);
    void ask(newest.map((c) => c.id));
  }, [chats, ask]);

  const polling = Object.keys(seen).some((id) => seen[id].running);
  useEffect(() => {
    if (!polling) return;
    const timer = setInterval(() => {
      const ids = Object.keys(seenRef.current).filter((id) => seenRef.current[id].running && !gone.current.has(id));
      if (ids.length > 0) void ask(ids);
    }, WATCH_MS);
    return () => clearInterval(timer);
  }, [polling, ask]);

  const forget = useCallback((id: string) => {
    gone.current.add(id);
    setSeen(({ [id]: _g, ...rest }) => rest);
  }, []);

  /** The card of this chat was answered or cleared in this session: its mark goes now. */
  const clear = useCallback(
    (id: string) => setSeen((cur) => (cur[id]?.waiting ? { ...cur, [id]: { ...cur[id], waiting: false } } : cur)),
    [],
  );

  return { clear, waitingIds: Object.keys(seen).filter((id) => seen[id].waiting), forget };
}
