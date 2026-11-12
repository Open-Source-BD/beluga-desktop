import { useEffect, useState } from "react";
import { listPacks, packLicenses, type LicenseFile, type PackView } from "../../lib/ipc";
import "../skills/skills.css";

interface Entry {
  pack: PackView;
  files: LicenseFile[];
}

/** Every pack with its license id and the text of its license files. */
export default function Licenses({ onBack }: { onBack: () => void }) {
  const [entries, setEntries] = useState<Entry[] | null>(null);

  useEffect(() => {
    let live = true;
    (async () => {
      let packs: PackView[] = [];
      try {
        packs = await listPacks();
      } catch {
        // An empty list is shown.
      }
      const all = await Promise.all(
        packs.map(async (pack) => ({ pack, files: await packLicenses(pack.id).catch(() => []) })),
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
          {(entries ?? []).map(({ pack, files }) => (
            <details key={pack.id}>
              <summary>
                <b>{pack.title}</b>
                <span>{pack.license}</span>
              </summary>
              {files.map((f) => (
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
