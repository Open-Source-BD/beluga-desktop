import { useState } from "react";
import type { PackOrigin, PackView, SkillView } from "../../lib/ipc";
import Switch from "./Switch";

const ORIGIN: Record<PackOrigin, string> = {
  built_in: "built in",
  installed: "installed",
  yours: "added by you",
};
const COLLAPSED = 3;

interface Props {
  pack: PackView;
  /** Skills to list (all of them, or the search matches). */
  skills: SkillView[];
  /** A search is active: show every match. */
  searching: boolean;
  onTogglePack: (on: boolean) => void;
  onToggleSkill: (skill: SkillView, on: boolean) => void;
  onOpen: (skill: SkillView) => void;
}

export default function PackCard({ pack, skills, searching, onTogglePack, onToggleSkill, onOpen }: Props) {
  const [expanded, setExpanded] = useState(false);
  const total = pack.skills.length;
  const on = pack.skills.filter((s) => s.on && !s.broken).length;
  const open = searching || expanded;
  const shown = open ? skills : skills.slice(0, COLLAPSED);
  return (
    <section className={pack.on ? "pack" : "pack off"} aria-label={pack.title}>
      <div className="ph">
        <b>{pack.title}</b>
        <span className="meta">
          <span>{`${total} skills, ${on} on`}</span>
          {pack.license && <span>{pack.license}</span>}
          <span>{ORIGIN[pack.origin]}</span>
        </span>
        <Switch label={`${pack.title} pack`} checked={pack.on} onChange={onTogglePack} />
      </div>
      {pack.error && <p className="pack-error">{pack.error}</p>}
      <ul className="sks">
        {shown.map((s) => (
          <li key={s.id} className={pack.on ? "sk" : "sk dim"}>
            <button type="button" className="nm" onClick={() => onOpen(s)}>
              {s.name}
            </button>
            {s.broken ? (
              <span className="ds broken">{s.broken}</span>
            ) : (
              <>
                <span className="ds">{s.description}</span>
                <Switch label={s.name} checked={s.on} onChange={(v) => onToggleSkill(s, v)} />
              </>
            )}
          </li>
        ))}
      </ul>
      {!searching && skills.length > COLLAPSED && (
        <button type="button" className="more" onClick={() => setExpanded(!expanded)}>
          {expanded ? "Show fewer" : `Show all ${skills.length}`}
        </button>
      )}
    </section>
  );
}
