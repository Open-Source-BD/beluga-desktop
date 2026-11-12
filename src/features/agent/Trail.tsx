import TrailLine from "./TrailLine";
import type { TrailEntry } from "./trailEntries";

interface Props {
  entries: TrailEntry[];
  /** One stable key per entry (the same before and after the call is stored). */
  keys?: string[];
  expanded?: ReadonlySet<string>;
  onToggle?: (key: string) => void;
}

/** The calls of one step of a task, as a list of quiet lines. */
export default function Trail({ entries, keys, expanded, onToggle }: Props) {
  if (entries.length === 0) return null;
  return (
    <ul className="trail" aria-label="Steps">
      {entries.map((e, i) => {
        const key = keys?.[i] ?? `${i}:${e.callId}`;
        return (
          <li key={key}>
            <TrailLine
              entry={e}
              lineKey={key}
              open={expanded ? expanded.has(key) : undefined}
              onToggle={onToggle}
            />
          </li>
        );
      })}
    </ul>
  );
}
