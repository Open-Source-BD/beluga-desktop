import { useEffect, useRef, useState } from "react";
import { backgroundOutput, stopBackground, type BgInfo } from "../../lib/ipc";
import { toUiError } from "../chat/errors";

interface Props {
  chatId: string;
  /** The chat's background commands; finished ones are not listed. */
  items: BgInfo[];
  onError: (message: string) => void;
}

/** The output of one command, as text, read again once a second while it is open. */
function Output({ chatId, id, version, ended }: { chatId: string; id: string; version: BgInfo[]; ended: boolean }) {
  const [text, setText] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const last = useRef(0);
  const reading = useRef(false);
  const live = useRef(true);
  const read = useRef<() => void>(() => {});
  read.current = () => {
    if (reading.current) return;
    reading.current = true;
    last.current = Date.now();
    const done = (t: string | null) => {
      reading.current = false;
      if (!live.current) return;
      setFailed(t === null);
      if (t !== null) setText(t);
    };
    try {
      backgroundOutput(chatId, id).then(done, () => done(null));
    } catch {
      done(null);
    }
  };
  useEffect(() => {
    live.current = true;
    read.current();
    // An ended command's output no longer changes: read it once more, then stop.
    if (ended) return () => { live.current = false; };
    const timer = setInterval(() => read.current(), 1000);
    return () => {
      live.current = false;
      clearInterval(timer);
    };
  }, [chatId, id, ended]);
  // The commands changed: read now, but never more than once a second.
  useEffect(() => {
    if (Date.now() - last.current >= 1000) read.current();
  }, [version]);
  if (failed && text === null) return <p className="bg-note">Could not read the output.</p>;
  return (
    <>
      <pre className="bg-out" tabIndex={0} aria-label="Output">
        {text ?? ""}
      </pre>
      {ended && <p className="bg-note">This command has ended.</p>}
    </>
  );
}

/** Running background commands above the message box. */
export default function Background({ chatId, items, onError }: Props) {
  const [open, setOpen] = useState<string | null>(null);
  // A command that ends while its output is open stays listed until the user closes it.
  const shown = items.filter((i) => i.running || i.id === open);
  if (shown.length === 0) return null;
  const stop = async (id: string) => {
    try {
      await stopBackground(chatId, id);
    } catch (e) {
      onError(toUiError(e).message);
    }
  };
  return (
    <ul className="bg-strip" aria-label="Background commands">
      {shown.map((b) => (
        <li key={b.id} className="bg-item">
          <div className="bg-row">
            <span className="bg-label">{b.running ? "Running:" : "Ended:"}</span>
            <button
              type="button"
              className="bg-cmd"
              aria-expanded={open === b.id}
              onClick={() => setOpen(open === b.id ? null : b.id)}
            >
              <code style={{ whiteSpace: "pre" }}>{b.command}</code>
            </button>
            {b.running && (
              <button type="button" className="btn bg-stop" onClick={() => void stop(b.id)}>
                Stop
              </button>
            )}
          </div>
          {open === b.id && <Output chatId={chatId} id={b.id} version={items} ended={!b.running} />}
        </li>
      ))}
    </ul>
  );
}
