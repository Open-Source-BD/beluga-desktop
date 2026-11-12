import { memo, useId, useState } from "react";
import { ChevronIcon } from "../shell/icons";
import { stateWord, type TrailEntry } from "./trailEntries";

/**
 * One tool call as a quiet line. With a detail (or output so far) it is a
 * button that opens it; without, plain text. Everything is shown as text.
 */
function TrailLine({ entry }: { entry: TrailEntry }) {
  const [open, setOpen] = useState(false);
  const panel = useId();
  const word = stateWord(entry.status);
  const body = entry.detail || entry.output || "";
  const label = (
    <>
      <span className="trail-text">{entry.summary}</span>
      {word && (
        <>
          {" "}
          <span className="trail-state" data-state={entry.status}>
            {word}
          </span>
        </>
      )}
    </>
  );
  if (!body) {
    return <div className="trail-line plain">{label}</div>;
  }
  return (
    <div className="trail-line" data-status={entry.status}>
      <button
        type="button"
        className="trail-btn"
        aria-expanded={open}
        aria-controls={open ? panel : undefined}
        onClick={() => setOpen((o) => !o)}
      >
        <span className="trail-chev" data-open={open} aria-hidden="true">
          <ChevronIcon size={11} />
        </span>
        <span>{label}</span>
      </button>
      {open && (
        <pre id={panel} className="trail-detail">
          {body}
        </pre>
      )}
    </div>
  );
}

export default memo(TrailLine);
