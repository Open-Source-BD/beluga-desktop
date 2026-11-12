import { useState, type KeyboardEvent, type MouseEvent, type ReactNode, type Ref } from "react";
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
  /** False for the first moments of a card: its buttons ignore activation and look disabled. */
  armed: boolean;
  /** The approve buttons stay off (the user has not yet seen all of what they approve). */
  lock?: boolean;
  onAnswer: (answer: Answer) => void;
  /** The decline button: the least dangerous action, which takes focus when the card appears. */
  safeRef: Ref<HTMLButtonElement>;
}

/** The note field's label. */
export const NOTE_LABEL = "Tell it what to do instead (optional)";

/** A card's buttons. A decline that takes a note reveals the field; Enter in it sends. */
export default function Choices({ choices, decline, busy, armed, lock = false, onAnswer, safeRef }: Props) {
  const [noting, setNoting] = useState(false);
  const [note, setNote] = useState("");
  /** Activation of a card button: not before it is armed, and never the repeat of a double click. */
  const press = (act: () => void) => (e: MouseEvent) => {
    if (!armed || e.detail > 1) return;
    act();
  };
  const sendNote = () => {
    if (busy || !armed) return;
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
            disabled={busy || lock}
            aria-disabled={!armed || undefined}
            onClick={press(() => onAnswer(c.answer))}
          >
            {c.label}
          </button>
        ))}
        <button
          type="button"
          className="card-btn"
          ref={safeRef}
          disabled={busy}
          aria-disabled={!armed || undefined}
          aria-expanded={decline.note ? noting : undefined}
          onClick={press(() => {
            if (decline.note) setNoting(true);
            else if (decline.answer) onAnswer(decline.answer);
          })}
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
          <button type="button" className="card-btn" disabled={busy} aria-disabled={!armed || undefined} onClick={press(sendNote)}>
            Send
          </button>
        </div>
      )}
    </>
  );
}
