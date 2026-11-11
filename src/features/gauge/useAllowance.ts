import { useCallback, useEffect, useRef, useState } from "react";
import { chatUsage, keyStatus, monthSpend, type ChatUsage, type KeyStatus } from "../../lib/ipc";

export interface Allowance {
  status: KeyStatus | null;
  monthSpend: number;
  usage: ChatUsage | null;
  /** The last status refresh failed; `status` is the last known value. */
  failed: boolean;
  refresh(): void;
}

/** Loads on mount, when the chat changes, on `refresh()` and when the window regains focus. */
export function useAllowance(chatId: string | null): Allowance {
  const [status, setStatus] = useState<KeyStatus | null>(null);
  const [month, setMonth] = useState(0);
  const [usage, setUsage] = useState<ChatUsage | null>(null);
  const [failed, setFailed] = useState(false);
  const chatRef = useRef(chatId);
  chatRef.current = chatId;

  const refresh = useCallback(() => {
    keyStatus().then(
      (s) => {
        setStatus(s);
        setFailed(false);
      },
      () => setFailed(true),
    );
    monthSpend().then(setMonth, () => {});
    const id = chatRef.current;
    if (id) {
      chatUsage(id).then(
        (u) => {
          if (chatRef.current === id) setUsage(u);
        },
        () => {},
      );
    }
  }, []);

  useEffect(() => {
    setUsage(null);
    refresh();
  }, [chatId, refresh]);

  useEffect(() => {
    window.addEventListener("focus", refresh);
    return () => window.removeEventListener("focus", refresh);
  }, [refresh]);

  return { status, monthSpend: month, usage, failed, refresh };
}
