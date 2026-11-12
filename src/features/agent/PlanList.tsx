import type { PlanItem, PlanState } from "../../lib/ipc";

const WORDS: Record<PlanState, string> = {
  done: "done",
  in_progress: "in progress",
  pending: "to do",
};

/** The model's checklist. Each item's state is said in words for screen readers. */
export default function PlanList({ items }: { items: PlanItem[] }) {
  if (items.length === 0) return null;
  return (
    <section className="plan" aria-label="Task checklist">
      <ul>
        {items.map((it, i) => (
          <li key={i} data-state={it.state}>
            <span className="plan-mark" aria-hidden="true" />
            <span className="plan-text">{it.text}</span>
            <span className="sr-only">, {WORDS[it.state]}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}
