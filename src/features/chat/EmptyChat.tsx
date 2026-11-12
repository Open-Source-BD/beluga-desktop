import type { ReactNode } from "react";

/** A suggested first message; picking it only fills the message box. */
export interface Starter {
  label: string;
  hint: string;
  onPick: () => void;
}

interface Props {
  /** The message box, shown under the greeting. */
  children: ReactNode;
  /** Shown under the box; empty or absent shows nothing. */
  starters?: Starter[];
}

/** The empty state: the greeting above the message box (children), then the starter actions. */
export default function EmptyChat({ children, starters = [] }: Props) {
  return (
    <div className="empty">
      <div className="col">
        <h2 className="hello">What are we building?</h2>
        <p className="hello-sub">
          Ask a question, paste an error, or think a problem through.
        </p>
      </div>
      {children}
      {starters.length > 0 && (
        <ul className="col starts" aria-label="Ways to start">
          {starters.map((s) => (
            <li key={s.label}>
              <button type="button" className="start" onClick={s.onPick}>
                <span className="start-w">{s.label}</span>
                <span className="start-s">{s.hint}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
