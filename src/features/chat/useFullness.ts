import { useEffect, useRef, useState } from "react";
import { chatFullness } from "../../lib/ipc";

const TYPING_DELAY_MS = 300;

/**
 * How full the chat is, as a percent, or null when unknown. Asked again when
 * the chat, model, skills or `turn` change, and 300 ms after the draft length
 * stops changing.
 */
export function useFullness(
  chatId: string | null,
  modelId: string | null,
  skillsKey: string,
  turn: number,
  draftChars: number,
): number | null {
  const [held, setHeld] = useState<{ chatId: string; percent: number | null } | null>(null);
  const draft = useRef(draftChars);
  draft.current = draftChars;
  const seq = useRef(0);

  const ask = (id: string, model: string, chars: number) => {
    const mine = ++seq.current;
    chatFullness(id, model, chars).then(
      (f) => {
        if (seq.current === mine) setHeld({ chatId: id, percent: f.context_length > 0 ? f.percent : null });
      },
      () => {},
    );
  };

  useEffect(() => {
    if (chatId && modelId) ask(chatId, modelId, draft.current);
  }, [chatId, modelId, skillsKey, turn]);

  const first = useRef(true);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    if (!chatId || !modelId) return;
    const t = setTimeout(() => ask(chatId, modelId, draftChars), TYPING_DELAY_MS);
    return () => clearTimeout(t);
  }, [draftChars]);

  return held && held.chatId === chatId ? held.percent : null;
}
