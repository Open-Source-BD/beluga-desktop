import type { ActiveSkill } from "../../lib/ipc";
import "./skills.css";

interface Props {
  skills: ActiveSkill[];
  onRemove: (id: string) => void;
}

/** Chips above the message box for the skills active in this chat. */
export default function ActiveSkills({ skills, onRemove }: Props) {
  if (skills.length === 0) return null;
  return (
    <ul className="active-skills" aria-label="Active skills">
      {skills.map((s) => (
        <li key={s.id} className="skill-chip">
          <span className="skill-chip-name">{s.name}</span>
          {s.unavailable ? (
            <span className="skill-chip-note">not available</span>
          ) : s.too_long ? (
            <span className="skill-chip-note">too long for this model</span>
          ) : null}
          <button
            type="button"
            className="skill-chip-x"
            aria-label={`Remove ${s.name}`}
            onClick={() => onRemove(s.id)}
          >
            <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" aria-hidden="true" focusable="false">
              <path d="M6 6l12 12M18 6 6 18" />
            </svg>
          </button>
        </li>
      ))}
    </ul>
  );
}
