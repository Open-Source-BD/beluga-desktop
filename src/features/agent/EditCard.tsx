import type { Ref } from "react";
import type { Answer, Approval } from "../../lib/ipc";
import Choices from "./Choices";
import { useScrollGate } from "./useScrollGate";

type Edit = Extract<Approval, { kind: "edit" }>;

interface Props {
  approval: Edit;
  busy: boolean;
  armed: boolean;
  onAnswer: (answer: Answer) => void;
  safeRef: Ref<HTMLButtonElement>;
}

const more = (total: number, shown: number) => Math.max(0, total - shown);

/** The change as the core described it; every line is plain text. */
export default function EditCard({ approval: a, busy, armed, onAnswer, safeRef }: Props) {
  const removedMore = more(a.removed_total, a.removed.length);
  const addedMore = more(a.added_total, a.added.length);
  const removedGate = useScrollGate(a.removed);
  const addedGate = useScrollGate(a.added);
  const gated = removedGate.gated || addedGate.gated;
  return (
    <>
      <h3 className="card-q" id={`card-${a.id}`}>
        {a.created ? "Create this file?" : "Change this file?"}
      </h3>
      <p className="card-line">
        <code className="card-mono" style={{ whiteSpace: "pre-wrap" }}>
          {a.path}
        </code>
      </p>
      {a.created && <p className="card-line">{`New file, ${a.total_lines} lines.`}</p>}
      {a.removed.length > 0 && (
        <div
          ref={removedGate.ref}
          onScroll={removedGate.onScroll}
          className="card-pre card-diff"
          role="group"
          aria-label="Lines removed"
          tabIndex={0}
        >
          {a.removed.map((l, i) => (
            <span key={i} className="diff-line removed" style={{ whiteSpace: "pre-wrap" }}>
              {`-${l}`}
            </span>
          ))}
        </div>
      )}
      {removedMore > 0 && (
        <p className="card-fine">{`${removedMore} more lines removed are not shown.`}</p>
      )}
      {a.added.length > 0 && (
        <div
          ref={addedGate.ref}
          onScroll={addedGate.onScroll}
          className="card-pre card-diff"
          role="group"
          aria-label="Lines added"
          tabIndex={0}
        >
          {a.added.map((l, i) => (
            <span key={i} className="diff-line added" style={{ whiteSpace: "pre-wrap" }}>
              {`+${l}`}
            </span>
          ))}
        </div>
      )}
      {addedMore > 0 && <p className="card-fine">{`${addedMore} more lines added are not shown.`}</p>}
      {gated && <p className="card-line">Scroll to read all of the change.</p>}
      <Choices
        choices={[
          { label: "Allow once", answer: { decision: "once" } },
          { label: "Allow all edits in this chat", answer: { decision: "all_edits" } },
        ]}
        decline={{ label: "Don't allow", note: true }}
        busy={busy}
        armed={armed}
        lock={gated}
        onAnswer={onAnswer}
        safeRef={safeRef}
      />
    </>
  );
}
