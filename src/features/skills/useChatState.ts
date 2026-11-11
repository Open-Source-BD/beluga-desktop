import { useCallback, useEffect, useRef, useState } from "react";
import { chatState, deactivateSkill, type ActiveSkill } from "../../lib/ipc";

/**
 * The chat's active skills and folder for the model in use. Refreshed when the chat or
 * model changes and on `refresh(chatId)`, which a caller uses after activating
 * a skill or when a turn ends.
 */
export function useChatState(chatId: string | null, modelId: string | null) {
  const [held, setHeld] = useState<{ id: string; skills: ActiveSkill[]; folder: string | null } | null>(null);
  const modelRef = useRef(modelId);
  modelRef.current = modelId;
  const seq = useRef(0);

  const refresh = useCallback(async (id: string, model: string | null = modelRef.current) => {
    if (!model) return;
    const mine = ++seq.current;
    try {
      const s = await chatState(id, model);
      if (seq.current === mine) setHeld({ id, skills: s.active_skills, folder: s.folder });
    } catch {
      // Chips are a convenience; keep what is shown.
    }
  }, []);

  useEffect(() => {
    if (chatId && modelId) void refresh(chatId, modelId);
  }, [chatId, modelId, refresh]);

  const remove = useCallback(
    async (skillId: string) => {
      if (!chatId) return;
      try {
        await deactivateSkill(chatId, skillId);
      } catch {
        // The chip stays; the refresh below shows the truth.
      }
      await refresh(chatId);
    },
    [chatId, refresh],
  );

  const skills = held && held.id === chatId ? held.skills : [];
  const folder = held && held.id === chatId ? held.folder : null;
  return { skills, folder, refresh, remove };
}
