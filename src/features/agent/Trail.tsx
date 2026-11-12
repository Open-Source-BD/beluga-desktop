import TrailLine from "./TrailLine";
import type { TrailEntry } from "./trailEntries";

/** The calls of one step of a task, as a list of quiet lines. */
export default function Trail({ entries }: { entries: TrailEntry[] }) {
  if (entries.length === 0) return null;
  return (
    <ul className="trail" aria-label="Steps">
      {entries.map((e) => (
        <li key={e.callId}>
          <TrailLine entry={e} />
        </li>
      ))}
    </ul>
  );
}
