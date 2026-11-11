import { useEffect, useRef, useState, type FormEvent } from "react";
import { APP_NAME } from "../../app/brand";
import {
  onAuthFinished,
  openAuthUrl,
  resolveKeyStorage,
  signInCancel,
  signInCodeUrl,
  signInStart,
  signInWithCode,
  signInWithKey,
  type AuthFinished,
  type StorageChoice,
  type UiError,
} from "../../lib/ipc";
import { WhaleMark } from "../shell/icons";
import KeyStorageChoice from "./KeyStorageChoice";
import "./signin.css";

interface Props {
  notice?: string;
  onSignedIn: () => void;
}

type Phase = "idle" | "waiting";
type PasteMode = "closed" | "key" | "code";

const FINISH_MESSAGES: Partial<Record<string, string>> = {
  denied: "OpenRouter sign-in was declined. Try again, or paste a key.",
  timed_out: "Sign-in took too long and was cancelled. Try again.",
  failed: "Sign-in did not complete. Try again, or paste a key.",
};

function messageOf(err: unknown): string {
  if (err && typeof err === "object" && "message" in err) {
    return String((err as UiError).message);
  }
  return "Something went wrong. Try again.";
}

function isKeychainUnavailable(err: unknown): boolean {
  return (
    !!err &&
    typeof err === "object" &&
    (err as UiError).kind === "keychain_unavailable"
  );
}

export default function SignIn({ notice, onSignedIn }: Props) {
  const [phase, setPhase] = useState<Phase>("idle");
  /** The browser sign-in page, kept so it can be opened again. */
  const [authUrl, setAuthUrl] = useState<string | null>(null);
  const [pasteMode, setPasteMode] = useState<PasteMode>("closed");
  const [secret, setSecret] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [keychainOpen, setKeychainOpen] = useState(false);
  const [keychainBusy, setKeychainBusy] = useState(false);
  const [keychainError, setKeychainError] = useState<string | null>(null);
  const onSignedInRef = useRef(onSignedIn);
  onSignedInRef.current = onSignedIn;

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    onAuthFinished((result: AuthFinished) => {
      if (result.ok) {
        onSignedInRef.current();
        return;
      }
      setPhase("idle");
      if (result.reason === "keychain_unavailable") {
        setKeychainOpen(true);
      } else if (result.reason && result.reason !== "cancelled") {
        setError(FINISH_MESSAGES[result.reason] ?? FINISH_MESSAGES.failed!);
      }
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  async function startBrowserSignIn() {
    setError(null);
    setAuthUrl(null);
    setPhase("waiting");
    try {
      const { auth_url } = await signInStart();
      setAuthUrl(auth_url);
    } catch (err) {
      setPhase("idle");
      setError(messageOf(err));
    }
  }

  async function cancel() {
    try {
      await signInCancel();
    } catch {
      // Cancelling a sign-in that already ended is harmless.
    }
    setPhase("idle");
  }

  async function openAgain() {
    if (!authUrl) return;
    try {
      await openAuthUrl(authUrl);
    } catch (err) {
      setError(messageOf(err));
    }
  }

  function switchToKey() {
    setError(null);
    setSecret("");
    setPasteMode("key");
  }

  async function switchToCode() {
    setError(null);
    try {
      const { auth_url } = await signInCodeUrl();
      await openAuthUrl(auth_url);
      setSecret("");
      setPasteMode("code");
    } catch (err) {
      setError(messageOf(err));
    }
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    const value = secret.trim();
    if (!value || submitting) return;
    setSecret("");
    setError(null);
    setSubmitting(true);
    try {
      if (pasteMode === "code") await signInWithCode(value);
      else await signInWithKey(value);
      onSignedInRef.current();
    } catch (err) {
      if (isKeychainUnavailable(err)) setKeychainOpen(true);
      else setError(messageOf(err));
    } finally {
      setSubmitting(false);
    }
  }

  async function chooseStorage(choice: StorageChoice) {
    setKeychainBusy(true);
    setKeychainError(null);
    try {
      await resolveKeyStorage(choice);
      setKeychainOpen(false);
      onSignedInRef.current();
    } catch (err) {
      setKeychainError(messageOf(err));
    } finally {
      setKeychainBusy(false);
    }
  }

  const isCode = pasteMode === "code";

  return (
    <div className="signin">
      <div className="signin-pitch">
        <div className="signin-brand">
          <WhaleMark />
          {APP_NAME}
        </div>
        <h2>A senior developer's habits, on a free model.</h2>
        <p>
          Plans before it codes, tests what it writes, and asks before it
          touches your machine.
        </p>
      </div>
      <div className="signin-form">
        <h1>Connect your OpenRouter account</h1>
        <p>
          {APP_NAME} has no subscription. It runs on your own OpenRouter
          account, which is free to open.
        </p>
        {notice && <p className="signin-notice">{notice}</p>}
        {error && (
          <p className="signin-error" role="alert">
            {error}
          </p>
        )}
        {phase === "waiting" ? (
          <div className="signin-waiting">
            <p role="status">Finish signing in in your browser.</p>
            {authUrl && (
              <button type="button" className="signin-link" onClick={openAgain}>
                Open the sign-in page again
              </button>
            )}
            <button type="button" className="signin-alt" onClick={cancel}>
              Cancel
            </button>
          </div>
        ) : (
          <>
            <button
              type="button"
              className="signin-big"
              onClick={startBrowserSignIn}
            >
              Sign in with OpenRouter
            </button>
            {pasteMode === "closed" ? (
              <button
                type="button"
                className="signin-alt"
                onClick={() => setPasteMode("key")}
              >
                Paste a key I already have
              </button>
            ) : (
              <form className="signin-paste" onSubmit={submit}>
                <label htmlFor="signin-secret">
                  {isCode ? "One-time code" : "OpenRouter key"}
                </label>
                <input
                  id="signin-secret"
                  key={pasteMode}
                  type={isCode ? "text" : "password"}
                  autoComplete="off"
                  spellCheck={false}
                  value={secret}
                  onChange={(e) => setSecret(e.target.value)}
                />
                <button
                  type="submit"
                  className="signin-alt"
                  disabled={submitting || !secret.trim()}
                >
                  {isCode ? "Use this code" : "Use this key"}
                </button>
                {isCode ? (
                  <>
                    <button
                      type="button"
                      className="signin-link"
                      onClick={switchToCode}
                    >
                      Get a new code
                    </button>
                    <button
                      type="button"
                      className="signin-link"
                      onClick={switchToKey}
                    >
                      Paste a key instead
                    </button>
                  </>
                ) : (
                  <button
                    type="button"
                    className="signin-link"
                    onClick={switchToCode}
                  >
                    Get a one-time code instead
                  </button>
                )}
              </form>
            )}
          </>
        )}
        <p className="signin-allowance">
          Free models allow 50 requests a day. A one-time $10 top-up on
          OpenRouter raises that to 1,000 a day.
        </p>
        <p className="signin-fine">
          Your key stays in this computer's keychain and is never sent to us.
        </p>
      </div>
      {keychainOpen && (
        <KeyStorageChoice
          busy={keychainBusy}
          error={keychainError}
          onChoose={chooseStorage}
        />
      )}
    </div>
  );
}
