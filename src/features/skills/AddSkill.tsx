import { useState } from "react";
import { cancelAdd, confirmAdd, previewAdd, type AddPreview, type AddSource } from "../../lib/ipc";
import { toUiError } from "../chat/errors";

interface Props {
  /** Closes the dialog; `added` is true after a confirmed add. */
  onClose: (added: boolean) => void;
}

export default function AddSkill({ onClose }: Props) {
  const [preview, setPreview] = useState<AddPreview | null>(null);
  const [github, setGithub] = useState(false);
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const find = async (source: AddSource) => {
    setBusy(true);
    setError(null);
    try {
      // The core opens the folder or ZIP dialog; null means the user cancelled.
      setPreview(await previewAdd(source));
    } catch (e) {
      setError(toUiError(e).message);
    } finally {
      setBusy(false);
    }
  };

  const cancel = () => {
    if (preview) void cancelAdd(preview.token).catch(() => {});
    onClose(false);
  };

  const confirm = async () => {
    if (!preview) return;
    setBusy(true);
    setError(null);
    try {
      await confirmAdd(preview.token);
      onClose(true);
    } catch (e) {
      setError(toUiError(e).message);
      setBusy(false);
    }
  };

  const n = preview?.skills.length ?? 0;
  return (
    <div
      className="scrim"
      onKeyDown={(e) => {
        if (e.key === "Escape") cancel();
      }}
    >
      <div className="dialog wide" role="dialog" aria-modal="true" aria-labelledby="add-title">
        <h3 id="add-title">Add a skill</h3>
        {error && (
          <p className="skills-error" role="alert">
            {error}
          </p>
        )}
        {preview ? (
          <>
            <ul className="found">
              {preview.skills.map((s) => (
                <li key={s.id}>
                  <b>{s.name}</b> <span>{s.description}</span>
                </li>
              ))}
            </ul>
            {preview.skipped.length > 0 && (
              <>
                <p className="skipped-head">Left out</p>
                <ul className="skipped">
                  {preview.skipped.map((f) => (
                    <li key={f}>{f}</li>
                  ))}
                </ul>
              </>
            )}
          </>
        ) : (
          <div className="choices">
            <button type="button" className="btn" disabled={busy} onClick={() => void find({ kind: "folder" })}>
              A folder on this computer
            </button>
            <button type="button" className="btn" disabled={busy} onClick={() => void find({ kind: "zip" })}>
              A ZIP file
            </button>
            <button type="button" className="btn" disabled={busy} onClick={() => setGithub(true)}>
              A GitHub link
            </button>
            {github && (
              <form
                className="gh-form"
                onSubmit={(e) => {
                  e.preventDefault();
                  if (url.trim()) void find({ kind: "github", url: url.trim() });
                }}
              >
                <input
                  aria-label="GitHub link"
                  value={url}
                  autoFocus
                  placeholder="https://github.com/owner/repo"
                  onChange={(e) => setUrl(e.target.value)}
                />
                <button type="submit" className="btn" disabled={busy || !url.trim()}>
                  Find skills
                </button>
              </form>
            )}
          </div>
        )}
        <div className="dialog-actions">
          <button type="button" className="btn" onClick={cancel}>
            Cancel
          </button>
          {preview && (
            <button type="button" className="btn danger" disabled={busy || n === 0} onClick={() => void confirm()}>
              {n === 1 ? "Add 1 skill" : `Add ${n} skills`}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
