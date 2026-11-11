import { useEffect, useRef, type KeyboardEvent } from "react";

interface Props {
  busy: boolean;
  error: string | null;
  onChoose: (choice: "session" | "file") => void;
}

/** Modal shown when the OS has no keychain. It closes only through its buttons. */
export default function KeyStorageChoice({ busy, error, onChoose }: Props) {
  const keepRef = useRef<HTMLButtonElement>(null);
  const fileRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    keepRef.current?.focus();
  }, []);

  function trapTab(e: KeyboardEvent) {
    if (e.key !== "Tab") return;
    const first = keepRef.current;
    const last = fileRef.current;
    if (!first || !last) return;
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  }

  return (
    <div className="keydlg-backdrop">
      <div
        className="keydlg"
        role="dialog"
        aria-modal="true"
        aria-labelledby="keydlg-title"
        aria-describedby="keydlg-body"
        onKeyDown={trapTab}
      >
        <h2 id="keydlg-title">This computer has no keychain service</h2>
        <p id="keydlg-body">
          Beluga can keep your key until you quit, or save it in a file only
          your account can read. A file is less protected than a keychain.
        </p>
        {error && (
          <p className="signin-error" role="alert">
            {error}
          </p>
        )}
        <div className="keydlg-actions">
          <button
            ref={keepRef}
            type="button"
            className="signin-big"
            disabled={busy}
            onClick={() => onChoose("session")}
          >
            Keep until I quit
          </button>
          <button
            ref={fileRef}
            type="button"
            className="signin-alt"
            disabled={busy}
            onClick={() => onChoose("file")}
          >
            Save in a file
          </button>
        </div>
      </div>
    </div>
  );
}
