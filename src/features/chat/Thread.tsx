import { useLayoutEffect, useRef } from "react";
import type { StoredMessage, UiError } from "../../lib/ipc";
import ErrorNotice from "./ErrorNotice";
import { shouldFollow } from "./follow";
import Markdown from "./Markdown";
import type { ChatStatus } from "./useChat";

interface Props {
  messages: StoredMessage[];
  pendingUser: string | null;
  streamingText: string;
  status: ChatStatus;
  error: UiError | null;
  onRetry: () => void;
  onSignInAgain: () => void;
  onPickModel: () => void;
}

export default function Thread({
  messages,
  pendingUser,
  streamingText,
  status,
  error,
  onRetry,
  onSignInAgain,
  onPickModel,
}: Props) {
  const scroller = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const streaming = status === "streaming";
  const last = messages[messages.length - 1];
  const showInterrupted =
    !streaming && (status === "interrupted" || last?.interrupted === true);

  // After every render: stay at the bottom unless the user scrolled up.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && following.current) el.scrollTop = el.scrollHeight;
  });

  return (
    <div
      className="thread"
      ref={scroller}
      onScroll={(e) => {
        const el = e.currentTarget;
        following.current = shouldFollow(
          el.scrollTop,
          el.clientHeight,
          el.scrollHeight,
        );
      }}
    >
      <div className="col thread-col">
        {messages.map((m) =>
          m.role === "user" ? (
            <div key={m.id} className="me">
              {m.text}
            </div>
          ) : (
            <div key={m.id} className="reply">
              <Markdown text={m.text} />
            </div>
          ),
        )}
        {pendingUser !== null && (
          <div className="me">{pendingUser}</div>
        )}
        <div className="reply live" aria-live="polite">
          {streaming && streamingText && <Markdown text={streamingText} />}
        </div>
        {showInterrupted && (
          <div className="notice">
            <p>Reply interrupted.</p>
            <button type="button" className="btn" onClick={onRetry}>
              Retry
            </button>
          </div>
        )}
        {error && (
          <ErrorNotice
            error={error}
            onSignInAgain={onSignInAgain}
            onPickModel={onPickModel}
            onRetry={onRetry}
          />
        )}
      </div>
    </div>
  );
}
