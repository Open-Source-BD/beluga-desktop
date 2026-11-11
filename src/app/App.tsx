import { useEffect, useState } from "react";
import SignIn from "../features/signin/SignIn";
import { authStatus, onAuthRevoked } from "../lib/ipc";

const REVOKED_NOTICE =
  "OpenRouter no longer accepts this key. Sign in again to continue. Your chats are still here.";

export default function App() {
  const [signedIn, setSignedIn] = useState<boolean | null>(null);
  const [notice, setNotice] = useState<string | undefined>();

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    authStatus()
      .then((s) => {
        if (!disposed) setSignedIn(s.signed_in);
      })
      .catch(() => {
        if (!disposed) setSignedIn(false);
      });
    onAuthRevoked(() => {
      setNotice(REVOKED_NOTICE);
      setSignedIn(false);
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  if (signedIn === null) return null;
  if (!signedIn) {
    return (
      <SignIn
        notice={notice}
        onSignedIn={() => {
          setNotice(undefined);
          setSignedIn(true);
        }}
      />
    );
  }
  // Task 10 replaces this placeholder with the real shell.
  return <div data-testid="shell" />;
}
