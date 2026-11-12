import { useEffect, useState } from "react";
import { listPacks, packLicenses, type LicenseFile, type PackView } from "../../lib/ipc";
import { toUiError } from "../chat/errors";
import "../skills/skills.css";

interface Entry {
  pack: PackView;
  files: LicenseFile[] | null;
}

/** Every pack with its license id and the text of its license files. */
export default function Licenses({ onBack }: { onBack: () => void }) {
  const [entries, setEntries] = useState<Entry[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    (async () => {
      let packs: PackView[] = [];
      try {
        packs = await listPacks();
      } catch (e) {
        if (live) setError(toUiError(e).message);
      }
      const all = await Promise.all(
        packs.map(async (pack) => ({ pack, files: await packLicenses(pack.id).catch(() => null) })),
      );
      if (live) setEntries(all);
    })();
    return () => {
      live = false;
    };
  }, []);

  return (
    <section className="main settings-view">
      <header className="head">
        <button type="button" className="btn" onClick={onBack}>
          Back to Settings
        </button>
        <span className="ttl">Open-source licenses</span>
      </header>
      <div className="settings-body">
        <div className="col licenses">
          {error && (
            <p className="skills-error" role="alert">
              {error}
            </p>
          )}
          {entries === null && <p className="empty">Loading...</p>}
          {(entries ?? []).map(({ pack, files }) => (
            <details key={pack.id}>
              <summary>
                <b>{pack.title}</b>
                <span>{pack.license}</span>
              </summary>
              {files === null && <p className="skills-error">Could not load the license text.</p>}
              {(files ?? []).map((f) => (
                <div key={f.name}>
                  <p className="license-name">{f.name}</p>
                  <pre>{f.text}</pre>
                </div>
              ))}
            </details>
          ))}
        </div>
      </div>
    </section>
  );
}
