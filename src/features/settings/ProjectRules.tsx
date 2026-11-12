import { useCallback, useEffect, useId, useState } from "react";
import { projectRules, removeProjectRule, type ProjectRules as Rules } from "../../lib/ipc";
import { toUiError } from "../chat/errors";

/** "Allowed commands": every project folder with the commands saved as always allowed. */
export default function ProjectRules() {
  const [folders, setFolders] = useState<Rules[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const ids = useId();

  const load = useCallback(async () => {
    try {
      setFolders(await projectRules());
    } catch {
      // Nothing to show until the core answers.
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  const remove = async (folder: string, rule: string) => {
    setError(null);
    try {
      await removeProjectRule(folder, rule);
    } catch (e) {
      setError(toUiError(e).message);
    }
    await load();
  };

  const shown = (folders ?? []).filter((f) => f.rules.length > 0);
  return (
    <div className="setting">
      <h2>Allowed commands</h2>
      {folders && shown.length === 0 && <p className="setting-line">No commands are always allowed.</p>}
      {shown.length > 0 && (
        <ul className="rules">
          {shown.map((f, fi) => (
            <li key={f.folder} className="rules-folder">
              <code className="rules-path" style={{ whiteSpace: "pre-wrap" }}>
                {f.folder}
              </code>
              <ul className="rules-list">
                {f.rules.map((r, ri) => (
                  <li key={r} className="rules-rule">
                    <code id={`${ids}-${fi}-${ri}`} style={{ whiteSpace: "pre-wrap" }}>
                      {r}
                    </code>
                    <button
                      type="button"
                      className="btn"
                      aria-describedby={`${ids}-${fi}-${ri}`}
                      onClick={() => void remove(f.folder, r)}
                    >
                      Remove
                    </button>
                  </li>
                ))}
              </ul>
            </li>
          ))}
        </ul>
      )}
      {error && (
        <p className="setting-error" role="alert">
          {error}
        </p>
      )}
      <p className="setting-line">A saved command runs without asking only in its simple forms.</p>
    </div>
  );
}
