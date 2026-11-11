import { useCallback, useEffect, useState } from "react";
import { listPacks, type PackView } from "../../lib/ipc";
import type { Row } from "../menu/match";

/** Skills the `/` menu offers: on, in a pack that is on, and not broken. */
export function skillRows(packs: PackView[]): Row[] {
  return packs.flatMap((p) =>
    p.skills
      .filter((s) => s.available)
      .map((s) => ({
        id: s.id,
        name: `/${s.id}`,
        description: s.description,
        source: p.origin === "yours" ? "Added by you" : p.title,
        group: "Skills",
      })),
  );
}

/** The menu's skill list: loaded on mount; call `refresh` after skills change. */
export function useSkills(): { rows: Row[]; refresh: () => void } {
  const [rows, setRows] = useState<Row[]>([]);
  const refresh = useCallback(() => {
    listPacks().then(
      (packs) => setRows(skillRows(packs)),
      () => {},
    );
  }, []);
  useEffect(refresh, [refresh]);
  return { rows, refresh };
}
