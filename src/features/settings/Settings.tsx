import { useEffect, useState } from "react";
import { getAppearance, setAppearance, type Appearance } from "../../app/appearance";
import { authStatus, signOut, type StorageKind } from "../../lib/ipc";
import { toUiError } from "../chat/errors";
import Switch from "../skills/Switch";
import "../skills/skills.css";
import Licenses from "./Licenses";
import { readSendWithModifier, writeSendWithModifier } from "./sendKey";
import "./settings.css";

interface Props {
  onBack: () => void;
  /** The key is gone: return to the sign-in screen. */
  onSignedOut: () => void;
  /** The send-key switch was flipped. */
  onSendKeyChange?: (sendWithModifier: boolean) => void;
}

const CHOICES: { value: Appearance; label: string }[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

const STORAGE_COPY: Record<StorageKind, string> = {
  keychain: "This computer's keychain",
  session: "Until you quit",
  file: "A file on this computer",
};

export default function Settings({ onBack, onSignedOut, onSendKeyChange }: Props) {
  const [sendWithModifier, setSendWithModifier] = useState(readSendWithModifier);
  const [appearance, setChoice] = useState<Appearance>(getAppearance);
  const [storage, setStorage] = useState<StorageKind | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [licenses, setLicenses] = useState(false);

  useEffect(() => {
    let live = true;
    authStatus().then(
      (s) => live && setStorage(s.storage),
      () => {},
    );
    return () => {
      live = false;
    };
  }, []);

  const choose = (a: Appearance) => {
    setAppearance(a);
    setChoice(a);
  };

  const confirmSignOut = async () => {
    setBusy(true);
    setError(null);
    try {
      await signOut();
      onSignedOut();
    } catch (e) {
      setError(toUiError(e).message);
      setConfirming(false);
    } finally {
      setBusy(false);
    }
  };

  if (licenses) return <Licenses onBack={() => setLicenses(false)} />;

  return (
    <section className="main settings-view">
      <header className="head">
        <button type="button" className="btn" onClick={onBack}>
          Back to chat
        </button>
        <span className="ttl">Settings</span>
      </header>
      <div className="settings-body">
        <div className="col settings-col">
          <div className="setting">
            <h2 id="appearance-label">Appearance</h2>
            <div className="seg" role="group" aria-labelledby="appearance-label">
              {CHOICES.map((c) => (
                <button
                  key={c.value}
                  type="button"
                  aria-pressed={appearance === c.value}
                  onClick={() => choose(c.value)}
                >
                  {c.label}
                </button>
              ))}
            </div>
          </div>
          <div className="setting">
            <h2>Message box</h2>
            <div className="setting-row">
              <span>Send with Cmd/Ctrl+Enter</span>
              <Switch
                label="Send with Cmd/Ctrl+Enter"
                checked={sendWithModifier}
                onChange={(on) => {
                  writeSendWithModifier(on);
                  setSendWithModifier(on);
                  onSendKeyChange?.(on);
                }}
              />
            </div>
            <p className="setting-line">Enter then adds a new line.</p>
          </div>
          <div className="setting">
            <h2>Account</h2>
            {storage && <p className="setting-line">{`Key storage: ${STORAGE_COPY[storage]}`}</p>}
            {error && (
              <p className="setting-error" role="alert">
                {error}
              </p>
            )}
            {confirming ? (
              <div className="setting-confirm">
                <p>Sign out? Your chats stay on this computer.</p>
                <div className="dialog-actions">
                  <button type="button" className="btn" onClick={() => setConfirming(false)}>
                    Cancel
                  </button>
                  <button type="button" className="btn danger" disabled={busy} onClick={confirmSignOut}>
                    Sign out
                  </button>
                </div>
              </div>
            ) : (
              <button type="button" className="btn" onClick={() => setConfirming(true)}>
                Sign out
              </button>
            )}
          </div>
          <div className="setting">
            <h2>About</h2>
            <button type="button" className="btn" onClick={() => setLicenses(true)}>
              Open-source licenses
            </button>
          </div>
        </div>
      </div>
    </section>
  );
}
