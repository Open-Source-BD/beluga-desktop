import { useState, type KeyboardEvent, type ReactNode, type Ref } from "react";
import type { Answer } from "../../lib/ipc";

export interface Choice {
  label: ReactNode;
  /** Plain-text name when `label` is not a string. */
  name?: string;
  answer: Answer;
}

interface Props {
  /** The buttons before the decline, in the card's visual order. */
  choices: Choice[];
  decline: { label: string; /** The decline opens a note field first. */ note: boolean; answer?: Answer };
  busy: boolean;
  onAnswer: (answer: Answer) => void;
  /** The decline button: the least dangerous action, which takes focus when the card appears. */
  safeRef: Ref<HTMLButtonElement>;
}

/** The note field's label. */
export const NOTE_LABEL = "Tell it what to do instead (optional)";

/** A card's buttons. A decline that takes a note reveals the field; Enter in it sends. */
export default function Choices({ choices, decline, busy, onAnswer, safeRef }: Props) {
  const [noting, setNoting] = useState(false);
  const [note, setNote] = useState("");
  const sendNote = () => {
    if (busy) return;
    const text = note.trim();
    onAnswer({ decision: "decline", note: text === "" ? null : text });
  };
  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key !== "Enter" || e.nativeEvent.isComposing) return;
    e.preventDefault();
    sendNote();
  };
  return (
    <>
      <div className="card-btns">
        {choices.map((c, i) => (
          <button
            key={i}
            type="button"
            className={i === 0 ? "card-btn primary" : "card-btn"}
            aria-label={c.name}
            disabled={busy}
            onClick={() => onAnswer(c.answer)}
          >
            {c.label}
          </button>
        ))}
        <button
          type="button"
          className="card-btn"
          ref={safeRef}
          disabled={busy}
          aria-expanded={decline.note ? noting : undefined}
          onClick={() => {
            if (decline.note) setNoting(true);
            else if (decline.answer) onAnswer(decline.answer);
          }}
        >
          {decline.label}
        </button>
      </div>
      {noting && (
        <div className="card-note">
          <input
            type="text"
            className="card-note-field"
            aria-label={NOTE_LABEL}
            placeholder={NOTE_LABEL}
            maxLength={4000}
            autoFocus
            disabled={busy}
            value={note}
            onChange={(e) => setNote(e.target.value)}
            onKeyDown={onKey}
          />
          <button type="button" className="card-btn" disabled={busy} onClick={sendNote}>
            Send
          </button>
        </div>
      )}
    </>
  );
}
