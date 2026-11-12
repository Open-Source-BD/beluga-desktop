import { memo, useId, useState } from "react";
import { ChevronIcon } from "../shell/icons";
import { stateWord, type TrailEntry } from "./trailEntries";

/**
 * One tool call as a quiet line. With a detail (or output so far) it is a
 * button that opens it; without, plain text. Everything is shown as text.
 */
function TrailLine({
  entry,
  lineKey,
  open: controlled,
  onToggle,
}: {
  entry: TrailEntry;
  /** With `open` and `onToggle`, the thread keeps the expansion (it survives a remount). */
  lineKey?: string;
  open?: boolean;
  onToggle?: (key: string) => void;
}) {
  const [own, setOwn] = useState(false);
  const open = controlled ?? own;
  const toggle = () => (onToggle && lineKey ? onToggle(lineKey) : setOwn((o) => !o));
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
        onClick={toggle}
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
