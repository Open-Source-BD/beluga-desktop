import { useEffect, useState } from "react";
import { getSkill, openExternal, removeSkill, type SkillDetail as Detail } from "../../lib/ipc";
import Markdown from "../chat/Markdown";
import { toUiError } from "../chat/errors";

export const ACTS_NOTE =
  "Written for an agent that can edit files and run commands. In chat it can guide you step by step, but it cannot act yet.";

interface Props {
  id: string;
  /** The skill is one the user added, so it can be removed. */
  removable: boolean;
  onBack: () => void;
  onUse: (id: string) => Promise<void>;
  onAttach: (id: string, file: string) => Promise<void>;
  /** The skill was removed. */
  onRemoved: () => void;
}

export default function SkillDetail({ id, removable, onBack, onUse, onAttach, onRemoved }: Props) {
  const [detail, setDetail] = useState<Detail | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    getSkill(id).then(
      (d) => live && setDetail(d),
      (e) => live && setError(toUiError(e).message),
    );
    return () => {
      live = false;
    };
  }, [id]);

  const guard = async (act: () => Promise<unknown>) => {
    setError(null);
    try {
      await act();
    } catch (e) {
      setError(toUiError(e).message);
    }
  };

  return (
    <div className="detail">
      <button type="button" className="btn" onClick={onBack}>
        Back to Skills
      </button>
      {error && (
        <p className="skills-error" role="alert">
          {error}
        </p>
      )}
      {detail && (
        <>
          <h2>{detail.skill.name}</h2>
          <p className="detail-meta">
            <span>{detail.pack_title}</span>
            {detail.license && <span>{detail.license}</span>}
            {detail.source_url && (
              <button type="button" className="linkish" onClick={() => void guard(() => openExternal(detail.source_url))}>
                View source
              </button>
            )}
          </p>
          {detail.skill.acts_on_computer && <p className="acts-note">{ACTS_NOTE}</p>}
          <div className="detail-actions">
            <button type="button" className="btn danger" onClick={() => void guard(() => onUse(detail.skill.id))}>
              Use in this chat
            </button>
            {removable && (
              <button
                type="button"
                className="btn"
                onClick={() => void guard(async () => {
                  await removeSkill(detail.skill.id);
                  onRemoved();
                })}
              >
                Remove
              </button>
            )}
          </div>
          <Markdown text={detail.body} />
          {detail.files.some((f) => f !== "SKILL.md") && (
            <ul className="files">
              {detail.files
                .filter((f) => f !== "SKILL.md")
                .map((f) => (
                  <li key={f}>
                    <span>{f}</span>
                    <button
                      type="button"
                      className="btn"
                      onClick={() => void guard(() => onAttach(detail.skill.id, f))}
                    >
                      Attach to this chat
                    </button>
                  </li>
                ))}
            </ul>
          )}
        </>
      )}
    </div>
  );
}
