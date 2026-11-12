import { useState, type ReactNode } from "react";
import { APP_NAME } from "../../app/brand";
import type { ChatSummary } from "../../lib/ipc";
import FolderGroup from "../mentions/FolderGroup";
import ConfirmDelete from "./ConfirmDelete";
import {
  GearIcon,
  PencilIcon,
  PlusIcon,
  SkillsIcon,
  TrashIcon,
  WhaleGlyph,
} from "./icons";

interface Props {
  chats: ChatSummary[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onNew: () => void;
  onRename: (id: string, title: string) => void;
  onDelete: (id: string) => void;
  onOpenSettings: () => void;
  /** The open chat's folder; null when it has none. */
  folder?: string | null;
  onOpenFolder?: () => void;
  onCloseFolder?: () => void;
  /** Opens the Skills screen; the row is hidden without it. */
  onOpenSkills?: () => void;
  /** Skills that are on, shown on the Skills row. */
  skillsOn?: number;
  skillsActive?: boolean;
  footer?: ReactNode;
  /** Chats with a card waiting for an answer. */
  needsYou?: readonly string[];
}

export default function Sidebar({
  chats,
  selectedId,
  onSelect,
  onNew,
  onRename,
  onDelete,
  onOpenSettings,
  folder = null,
  onOpenFolder,
  onCloseFolder,
  onOpenSkills,
  skillsOn,
  skillsActive = false,
  footer,
  needsYou = [],
}: Props) {
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [confirming, setConfirming] = useState<string | null>(null);
  const sorted = [...chats].sort((a, b) => b.updated_at - a.updated_at);

  const commit = (chat: ChatSummary) => {
    const title = draft.trim();
    setEditing(null);
    if (title && title !== chat.title) onRename(chat.id, title);
  };

  return (
    <aside className="side">
      <div className="brand">
        <WhaleGlyph />
        {APP_NAME}
      </div>
      <button type="button" className="newc" onClick={onNew}>
        <PlusIcon />
        New chat
      </button>
      {onOpenFolder && (
        <FolderGroup folder={folder} onOpen={onOpenFolder} onClose={() => onCloseFolder?.()} />
      )}
      <nav className="side-list" aria-label="Chats">
        <div className="grp">Earlier</div>
        <ul>
          {sorted.map((chat) => (
            <li key={chat.id} className={chat.id === selectedId ? "row on" : "row"}>
              {editing === chat.id ? (
                <input
                  className="rename"
                  aria-label="Chat name"
                  autoFocus
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  onBlur={() => commit(chat)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") commit(chat);
                    if (e.key === "Escape") setEditing(null);
                  }}
                />
              ) : (
                <>
                  <button
                    type="button"
                    className="row-main"
                    aria-current={chat.id === selectedId ? "true" : undefined}
                    onClick={() => onSelect(chat.id)}
                  >
                    {chat.title}
                  </button>
                  {needsYou.includes(chat.id) && <span className="needs-you">needs you</span>}
                  <button
                    type="button"
                    className="row-act"
                    aria-label={`Rename ${chat.title}`}
                    onClick={() => {
                      setDraft(chat.title);
                      setEditing(chat.id);
                    }}
                  >
                    <PencilIcon />
                  </button>
                  <button
                    type="button"
                    className="row-act"
                    aria-label={`Delete ${chat.title}`}
                    onClick={() => setConfirming(chat.id)}
                  >
                    <TrashIcon />
                  </button>
                </>
              )}
            </li>
          ))}
        </ul>
        {onOpenSkills && (
          <button
            type="button"
            className={skillsActive ? "row settings on" : "row settings"}
            aria-current={skillsActive ? "page" : undefined}
            onClick={onOpenSkills}
          >
            <SkillsIcon />
            Skills
            {skillsOn !== undefined && <span className="n">{`${skillsOn} on`}</span>}
          </button>
        )}
        <button type="button" className="row settings" onClick={onOpenSettings}>
          <GearIcon />
          Settings
        </button>
      </nav>
      {footer && <div className="side-footer">{footer}</div>}
      {confirming && (
        <ConfirmDelete
          onKeep={() => setConfirming(null)}
          onDelete={() => {
            onDelete(confirming);
            setConfirming(null);
          }}
        />
      )}
    </aside>
  );
}
