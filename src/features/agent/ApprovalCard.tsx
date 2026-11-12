import { useEffect, useRef, useState } from "react";
import type { Answer, Approval } from "../../lib/ipc";
import { armDelay } from "./armDelay";
import CommandCard from "./CommandCard";
import EditCard from "./EditCard";
import KeepGoingCard from "./KeepGoingCard";
import SecretCard from "./SecretCard";

interface Props {
  approval: Approval;
  /** An answer is on its way (or was accepted): every button stays off until the card clears or is refused. */
  busy: boolean;
  /** Why the core refused the last answer. */
  error: string | null;
  onAnswer: (answer: Answer) => void;
  /** Esc and the Stop task button: stop the task. */
  onStop: () => void;
}

/**
 * Focus may move to a new card only from places where nothing is being done: nothing focused,
 * the message box while empty, or the conversation. Never from another input, a dialog, the
 * model picker or the sidebar.
 */
function mayTakeFocus(): boolean {
  const a = document.activeElement;
  if (!a || a === document.body) return true;
  if (a instanceof HTMLTextAreaElement && a.closest(".composer") !== null) return a.value === "";
  return a.closest(".thread") !== null;
}

/**
 * The open card, one at a time, amber for a command. The decline button (the least dangerous
 * action) takes focus when the card appears when that is safe; otherwise the card is announced.
 * For a short moment after it appears every button ignores activation, so a click meant for the
 * card before it cannot land on this one.
 */
export default function ApprovalCard({ approval, busy, error, onAnswer, onStop }: Props) {
  const safe = useRef<HTMLButtonElement>(null);
  const [armed, setArmed] = useState(() => armDelay() <= 0);
  const [announce, setAnnounce] = useState(false);
  useEffect(() => {
    if (mayTakeFocus()) safe.current?.focus();
    else setAnnounce(true);
    if (armDelay() <= 0) return;
    const t = setTimeout(() => setArmed(true), armDelay());
    return () => clearTimeout(t);
    // Once per card (the card is keyed by its id).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [approval.id]);
  const body = (() => {
    const p = { busy, armed, onAnswer, safeRef: safe };
    switch (approval.kind) {
      case "command":
        return <CommandCard approval={approval} {...p} />;
      case "edit":
        return <EditCard approval={approval} {...p} />;
      case "secret":
        return <SecretCard approval={approval} {...p} />;
      case "keep_going":
        return <KeepGoingCard approval={approval} {...p} />;
    }
  })();
  return (
    <section
      className={`card card-${approval.kind}`}
      role="group"
      aria-labelledby={`card-${approval.id}`}
      onKeyDown={(e) => {
        if (e.key === "Escape" && !e.nativeEvent.isComposing) {
          e.preventDefault();
          onStop();
        }
      }}
    >
      {body}
      {error && (
        <p className="card-error" role="alert">
          {error}
        </p>
      )}
      <div className="card-foot">
        <button
          type="button"
          className="card-stop"
          disabled={busy}
          aria-disabled={!armed || undefined}
          onClick={(e) => {
            if (armed && e.detail <= 1) onStop();
          }}
        >
          Stop task
        </button>
      </div>
      <p className="sr-only" role="status">
        {announce ? "A request needs your answer." : ""}
      </p>
    </section>
  );
}
