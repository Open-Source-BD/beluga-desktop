import { useLayoutEffect, useRef } from "react";
import type { StoredMessage, UiError } from "../../lib/ipc";
import { PencilIcon } from "../shell/icons";
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
  /** The pending text was rejected before it was stored. */
  notSent?: boolean;
  /** Puts a user message back in the box for editing; omitted, no pencil shows. */
  onEdit?: (message: StoredMessage) => void;
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
  notSent = false,
  onEdit,
  onRetry,
  onSignInAgain,
  onPickModel,
}: Props) {
  const scroller = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const streaming = status === "streaming";
  const last = messages.filter((m) => m.kind === "message").pop();
  const canEdit = onEdit !== undefined && !streaming && pendingUser === null;
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
          m.kind === "note" ? (
            <p key={m.id} className="note-line">
              {m.text}
            </p>
          ) : m.role === "user" ? (
            <div key={m.id} className="me">
              {canEdit && (
                <button
                  type="button"
                  className="me-edit"
                  aria-label="Edit message"
                  onClick={() => onEdit(m)}
                >
                  <PencilIcon size={12} />
                </button>
              )}
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
            </div>
          ) : (
            <div key={m.id} className="reply">
              <Markdown text={m.text} />
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
  );
}
