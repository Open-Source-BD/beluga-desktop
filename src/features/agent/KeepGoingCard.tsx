import type { Ref } from "react";
import { APP_NAME } from "../../app/brand";
import type { Answer, Approval } from "../../lib/ipc";
import Choices from "./Choices";

type KeepGoing = Extract<Approval, { kind: "keep_going" }>;

interface Props {
  approval: KeepGoing;
  busy: boolean;
  onAnswer: (answer: Answer) => void;
  safeRef: Ref<HTMLButtonElement>;
}

/** "12 free requests left today.", "Spent $0.50 today." or nothing when the core knows neither. */
function status(a: KeepGoing): string {
  if (a.free_left !== null) return `${a.free_left} free requests left today.`;
  if (a.spent_today_usd !== null) return `Spent $${a.spent_today_usd.toFixed(2)} today.`;
  return "";
}

export default function KeepGoingCard({ approval: a, busy, onAnswer, safeRef }: Props) {
  const s = status(a);
  return (
    <>
      <p className="card-q" id={`card-${a.id}`}>
        {`${APP_NAME} has made ${a.requests} requests on this task.${s ? ` ${s}` : ""}`}
      </p>
      <Choices
        choices={[{ label: "Keep going", answer: { decision: "keep_going" } }]}
        decline={{ label: "Stop here", note: false, answer: { decision: "stop_here" } }}
        busy={busy}
        onAnswer={onAnswer}
        safeRef={safeRef}
      />
    </>
  );
}
