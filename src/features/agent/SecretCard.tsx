import type { Ref } from "react";
import { APP_NAME } from "../../app/brand";
import type { Answer, Approval } from "../../lib/ipc";
import Choices from "./Choices";

type Secret = Extract<Approval, { kind: "secret" }>;

interface Props {
  approval: Secret;
  busy: boolean;
  armed: boolean;
  onAnswer: (answer: Answer) => void;
  safeRef: Ref<HTMLButtonElement>;
}

export default function SecretCard({ approval: a, busy, armed, onAnswer, safeRef }: Props) {
  return (
    <>
      <p className="card-q" id={`card-${a.id}`}>
        {`Let ${APP_NAME} read `}
        <code className="card-mono" style={{ whiteSpace: "pre-wrap" }}>
          {a.path}
        </code>
        ? Its contents will be sent to the model.
      </p>
      <Choices
        choices={[{ label: "Allow once", answer: { decision: "once" } }]}
        decline={{ label: "Don't allow", note: true }}
        busy={busy}
        armed={armed}
        onAnswer={onAnswer}
        safeRef={safeRef}
      />
    </>
  );
}
