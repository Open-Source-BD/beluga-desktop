import type { Ref } from "react";
import { APP_NAME } from "../../app/brand";
import type { Answer, Approval } from "../../lib/ipc";
import Choices from "./Choices";

type Command = Extract<Approval, { kind: "command" }>;

interface Props {
  approval: Command;
  busy: boolean;
  onAnswer: (answer: Answer) => void;
  safeRef: Ref<HTMLButtonElement>;
}

/**
 * The command exactly as the core sent it: a plain text node in a block that keeps every
 * space and line break and scrolls sideways instead of wrapping or cutting anything.
 */
export default function CommandCard({ approval: a, busy, onAnswer, safeRef }: Props) {
  return (
    <>
      <h3 className="card-q" id={`card-${a.id}`}>
        Run this command?
      </h3>
      <pre className="card-pre card-command" style={{ whiteSpace: "pre" }} tabIndex={0} aria-label="Command">
        {a.command}
      </pre>
      <p className="card-line">
        <span className="card-key">Folder</span>{" "}
        <code className="card-mono" style={{ whiteSpace: "pre-wrap" }}>
          {a.folder}
        </code>
      </p>
      {a.background && <p className="card-line">This command keeps running in the background until it is stopped.</p>}
      {a.outside && <p className="card-line">This command refers to a place outside the project folder.</p>}
      {a.first_in_project && (
        <p className="card-line">{`Commands run on your computer with your permissions. ${APP_NAME} cannot undo them.`}</p>
      )}
      <Choices
        choices={[
          { label: "Run once", answer: { decision: "once" } },
          ...(a.rule !== null
            ? [
                {
                  label: (
                    <>
                      Always allow <code className="card-mono">{a.rule}</code> in this project
                    </>
                  ),
                  name: `Always allow ${a.rule} in this project`,
                  answer: { decision: "always" } as Answer,
                },
              ]
            : []),
        ]}
        decline={{ label: "Don't run", note: true }}
        busy={busy}
        onAnswer={onAnswer}
        safeRef={safeRef}
      />
      {a.rule !== null && (
        <p className="card-fine">This covers only simple forms of that command. Anything else still asks.</p>
      )}
    </>
  );
}
