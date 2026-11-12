import { useEffect, useRef } from "react";
import type { Answer, Approval } from "../../lib/ipc";
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
  /** Esc: stop the task. */
  onStop: () => void;
}

/** Whether focus sits in the message box with something typed: it is not taken away. */
function typingInBox(): boolean {
  const a = document.activeElement;
  return a instanceof HTMLTextAreaElement && a.closest(".composer") !== null && a.value !== "";
}

/**
 * The open card, one at a time, amber for a command. The decline button (the least dangerous
 * action) takes focus when the card appears, unless the user is typing a message.
 */
export default function ApprovalCard({ approval, busy, error, onAnswer, onStop }: Props) {
  const safe = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!typingInBox()) safe.current?.focus();
    // Once per card.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [approval.id]);
  const body = (() => {
    const p = { busy, onAnswer, safeRef: safe };
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
      key={approval.id}
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
    </section>
  );
}
