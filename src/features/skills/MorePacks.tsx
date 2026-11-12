import { useCallback, useEffect, useState } from "react";
import { extraPacks, installExtraPack, removePack, type ExtraPack } from "../../lib/ipc";
import { toUiError } from "../chat/errors";
import RedWarning from "./RedWarning";

interface Props {
  /** Packs were installed or removed. */
  onChanged: () => void;
}

export default function MorePacks({ onChanged }: Props) {
  const [packs, setPacks] = useState<ExtraPack[] | null>(null);
  const [warning, setWarning] = useState<ExtraPack | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setPacks(await extraPacks());
    } catch (e) {
      setError(toUiError(e).message);
      setPacks((p) => p ?? []);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  const run = async (id: string, act: () => Promise<unknown>) => {
    setBusy(id);
    setError(null);
    try {
      await act();
      await load();
      onChanged();
    } catch (e) {
      setError(toUiError(e).message);
    } finally {
      setBusy(null);
    }
  };

  const install = (p: ExtraPack, acknowledged: boolean) =>
    run(p.id, () => installExtraPack(p.id, acknowledged));

  return (
    <div className="more-packs">
      {error && (
        <p className="skills-error" role="alert">
          {error}
        </p>
      )}
      <ul className="extras">
        {(packs ?? []).map((p) => (
          <li key={p.id} className="pack extra">
            <div className="ph">
              <b>{p.title}</b>
              <span className="meta">
                <span>{p.license}</span>
                <span>{p.repo}</span>
              </span>
              {p.installed ? (
                <button type="button" className="btn" disabled={busy === p.id} onClick={() => run(p.id, () => removePack(p.id))}>
                  Remove
                </button>
              ) : (
                <button
                  type="button"
                  className="btn"
                  disabled={busy === p.id}
                  onClick={() => (p.needs_warning ? setWarning(p) : void install(p, false))}
                >
                  Install
                </button>
              )}
            </div>
            <p className="about">{p.about}</p>
          </li>
        ))}
      </ul>
      {warning && (
        <RedWarning
          onCancel={() => setWarning(null)}
          onInstall={() => {
            const p = warning;
            setWarning(null);
            void install(p, true);
          }}
        />
      )}
    </div>
  );
}
