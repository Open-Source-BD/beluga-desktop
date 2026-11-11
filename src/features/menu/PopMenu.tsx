import { useEffect, useRef, type KeyboardEvent, type ReactNode } from "react";
import type { Row } from "./match";
import "./menu.css";

export type PickMode = "run" | "complete";

interface Props {
  /** Already ordered group by group: this is also the keyboard order. */
  rows: Row[];
  groups: string[];
  active: number;
  onActiveChange: (index: number) => void;
  onPick: (row: Row, mode: PickMode) => void;
  onClose: () => void;
  footer?: ReactNode;
  /** "of N" shown in each group's heading. */
  totals?: Record<string, number>;
  /** Distinguishes option ids when more than one menu exists. */
  idPrefix?: string;
}

interface KeyActions {
  rows: Row[];
  active: number;
  onActiveChange: (index: number) => void;
  onPick: (row: Row, mode: PickMode) => void;
  onClose: () => void;
}

/** Handles the menu's keys; true when the key was one of them. Also used by the box that keeps focus. */
export function handleMenuKey(e: { key: string }, a: KeyActions): boolean {
  const last = a.rows.length - 1;
  switch (e.key) {
    case "ArrowDown":
      a.onActiveChange(Math.min(a.active + 1, last));
      return true;
    case "ArrowUp":
      a.onActiveChange(Math.max(a.active - 1, 0));
      return true;
    case "Enter":
    case "Tab": {
      const row = a.rows[a.active];
      if (!row) return false;
      a.onPick(row, e.key === "Enter" ? "run" : "complete");
      return true;
    }
    case "Escape":
      a.onClose();
      return true;
    default:
      return false;
  }
}

export function optionId(prefix: string, index: number): string {
  return `${prefix}-opt-${index}`;
}

export default function PopMenu({
  rows,
  groups,
  active,
  onActiveChange,
  onPick,
  onClose,
  footer,
  totals,
  idPrefix = "pop",
}: Props) {
  const root = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  // A press outside the menu's container (the message box) closes it.
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const boundary = root.current?.parentElement ?? root.current;
      if (boundary && !boundary.contains(e.target as Node)) closeRef.current();
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, []);

  useEffect(() => {
    document.getElementById(optionId(idPrefix, active))?.scrollIntoView?.({ block: "nearest" });
  }, [active, idPrefix]);

  const onKeyDown = (e: KeyboardEvent) => {
    if (handleMenuKey(e, { rows, active, onActiveChange, onPick, onClose })) e.preventDefault();
  };

  return (
    <div className="pop" ref={root}>
      <div
        role="listbox"
        id={`${idPrefix}-listbox`}
        aria-label="Commands and skills"
        tabIndex={-1}
        className="pop-list"
        onKeyDown={onKeyDown}
      >
        {groups.map((g) => {
          const inGroup = rows.map((r, i) => ({ r, i })).filter(({ r }) => r.group === g);
          if (inGroup.length === 0) return null;
          return (
            <div role="group" aria-label={g} key={g}>
              <div className="pop-hd" aria-hidden="true">
                <span>{g}</span>
                {totals?.[g] !== undefined && <span>{`${inGroup.length} of ${totals[g]}`}</span>}
              </div>
              {inGroup.map(({ r, i }) => (
                <div
                  key={r.id}
                  id={optionId(idPrefix, i)}
                  role="option"
                  aria-selected={i === active}
                  className={`pop-opt${i === active ? " on" : ""}`}
                  onMouseDown={(e) => e.preventDefault()}
                  onMouseMove={() => i !== active && onActiveChange(i)}
                  onClick={() => onPick(r, "run")}
                >
                  <span className="pop-name">{r.name}</span>
                  <span className="pop-desc">{r.description}</span>
                  <span className="pop-src">{r.source}</span>
                </div>
              ))}
            </div>
          );
        })}
      </div>
      {footer && <div className="pop-ft">{footer}</div>}
    </div>
  );
}
