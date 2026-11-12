import { useId, useLayoutEffect, useRef, useState } from "react";
import type { StoredMessage, UiError } from "../../lib/ipc";
import ErrorNotice from "./ErrorNotice";
import { shouldFollow } from "./follow";
import JumpToLatest from "./JumpToLatest";
import Markdown from "./Markdown";
import MessageActions from "./MessageActions";
import type { ChatStatus } from "./useChat";

interface Props {
  messages: StoredMessage[];
  pendingUser: string | null;
  streamingText: string;
  status: ChatStatus;
  error: UiError | null;
  /** The pending text was rejected before it was stored. */
  notSent?: boolean;
  /** Puts a user message back in the box for editing; omitted, no pencil shows. */
  onEdit?: (message: StoredMessage) => void;
  /** Answers the latest reply again (Retry in its row); omitted, the row has no Retry. */
  onRetryReply?: () => void;
  /** Opens the model picker to answer the latest reply again with another model. */
  onRetryWithModel?: () => void;
  /** The retry after a failure or an interruption. */
  onRetry: () => void;
  onSignInAgain: () => void;
  onPickModel: () => void;
}

/** A message's local date and time. */
const when = (m: StoredMessage) => new Date(m.created_at * 1000).toLocaleString();

export default function Thread({
  messages,
  pendingUser,
  streamingText,
  status,
  error,
  notSent = false,
  onEdit,
  onRetryReply,
  onRetryWithModel,
  onRetry,
  onSignInAgain,
  onPickModel,
}: Props) {
  const timeId = useId();
  const [away, setAway] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const streaming = status === "streaming";
  const last = messages.filter((m) => m.kind === "message").pop();
  const latestReply = last?.role === "assistant" ? last : undefined;
  const canEdit = onEdit !== undefined && !streaming && pendingUser === null;
  const showInterrupted =
    !streaming && (status === "interrupted" || last?.interrupted === true);

  // After every render: stay at the bottom unless the user scrolled up.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && following.current) el.scrollTop = el.scrollHeight;
  });

  const jump = () => {
    const el = scroller.current;
    if (el) el.scrollTop = el.scrollHeight;
    following.current = true;
    setAway(false);
  };

  return (
    <div className="thread-wrap">
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
        setAway(!following.current);
      }}
    >
      <div className="col thread-col">
        {messages.map((m) =>
          m.kind === "note" ? (
            <p key={m.id} className="note-line">
              {m.text}
            </p>
          ) : m.role === "user" ? (
            <div key={m.id} className="me" title={when(m)} aria-describedby={`${timeId}-${m.id}`}>
              <span id={`${timeId}-${m.id}`} className="sr-only">
                {when(m)}
              </span>
              {m.text}
              {m.attachments.length > 0 && (
                <ul className="me-files" aria-label="Attachments">
                  {m.attachments.map((a, i) => (
                    <li key={`${a.name}-${i}`} className="attach-chip">
                      {a.name}
                    </li>
                  ))}
                </ul>
              )}
              <MessageActions
                role="user"
                message={m}
                isLatest={false}
                onEdit={canEdit ? () => onEdit(m) : undefined}
              />
            </div>
          ) : (
            <div key={m.id} className="reply" title={when(m)} aria-describedby={`${timeId}-${m.id}`}>
              <span id={`${timeId}-${m.id}`} className="sr-only">
                {when(m)}
              </span>
              <Markdown text={m.text} />
              <MessageActions
                role="assistant"
                message={m}
                isLatest={m === latestReply}
                running={streaming || showInterrupted || pendingUser !== null}
                onRetry={onRetryReply}
                onRetryWithModel={onRetryWithModel}
              />
            </div>
          ),
        )}
        {pendingUser !== null && (
          <div className="me">
            {pendingUser}
            {notSent && <span className="not-sent"> Not sent.</span>}
          </div>
        )}
        <div className="reply live" aria-live="polite">
          {streaming &&
            (streamingText ? (
              <Markdown text={streamingText} />
            ) : (
              <p className="waiting">Waiting for the model…</p>
            ))}
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
    {away && <JumpToLatest onClick={jump} />}
    </div>
  );
}
