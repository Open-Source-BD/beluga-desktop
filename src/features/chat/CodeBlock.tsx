import { copyLabel, useCopy } from "./useCopy";

interface Props {
  /** The fence's language, or "" when it has none. */
  language: string;
  /** Exactly the code, without the fence's final newline. */
  code: string;
}

/** A fenced code block: the language, a "Copy code" button, and the code in the mono font. */
export default function CodeBlock({ language, code }: Props) {
  const { state, copy } = useCopy();
  return (
    <div className="codeblock">
      <div className="codeblock-head">
        <span className="codeblock-lang">{language || "text"}</span>
        <button type="button" className="codeblock-copy" onClick={() => void copy(code)}>
          {copyLabel(state, "Copy code")}
        </button>
      </div>
      <pre>
        <code>{code}</code>
      </pre>
    </div>
  );
}
