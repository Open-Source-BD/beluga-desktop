import { Fragment, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { StoredMessage, UiError } from "../../lib/ipc";
import Trail from "../agent/Trail";
import { bareEntry, resolvedEntries, type TrailEntry } from "../agent/trailEntries";
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
  /** Tool calls of the running task, from events. */
  live?: TrailEntry[];
  /** Seconds of a rate-limit wait being served. */
  waiting?: number | null;
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

const NO_LIVE: TrailEntry[] = [];

/** A message's local date and time. */
const when = (m: StoredMessage) => new Date(m.created_at * 1000).toLocaleString();

export default function Thread({
  messages,
  pendingUser,
  streamingText,
  status,
  error,
  live = NO_LIVE,
  waiting = null,
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
  // Copy and Retry belong to the last reply that has text, unless a message of the user follows it.
  const latestReply = useMemo(() => {
    const rows = messages.filter((m) => m.kind === "message");
    const l = rows[rows.length - 1];
    if (l?.role === "user") return undefined;
    return [...rows].reverse().find((m) => m.role === "assistant" && m.text !== "");
  }, [messages]);
  // A task's reply cannot be asked again as a whole: the core refuses it.
  const hasTools = useMemo(() => {
    let seen = false;
    for (const m of messages) {
      if (m.kind === "message" && m.role === "user") seen = false;
      else if (m.kind === "tool") seen = true;
    }
    return seen;
  }, [messages]);

  // Stored calls take their result from the stored tool rows, else from the live list
  // (a command still running), else from their name alone. Live lines already stored leave.
  const resolved = useMemo(() => resolvedEntries(messages), [messages]);
  const bare = useMemo(() => {
    const out = new Map<string, TrailEntry>();
    for (const m of messages)
      for (const c of m.tool_calls) if (!resolved.has(c.id)) out.set(c.id, bareEntry(c, streaming));
    return out;
  }, [messages, resolved, streaming]);
  const liveById = useMemo(() => new Map(live.map((e) => [e.callId, e])), [live]);
  const liveOnly = useMemo(() => {
    const stored = new Set<string>();
    for (const m of messages) for (const c of m.tool_calls) stored.add(c.id);
    return live.filter((e) => !stored.has(e.callId));
  }, [messages, live]);
  const entriesOf = (m: StoredMessage) =>
    m.tool_calls.map((c) => resolved.get(c.id) ?? liveById.get(c.id) ?? bare.get(c.id)!);
  const anyRunning = live.some((e) => e.status === "running");
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
          m.kind === "tool" ? null : m.kind === "note" ? (
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
            <Fragment key={m.id}>
              {(m.text !== "" || m.tool_calls.length === 0) && (
                <div className="reply" title={when(m)} aria-describedby={`${timeId}-${m.id}`}>
                  <span id={`${timeId}-${m.id}`} className="sr-only">
                    {when(m)}
                  </span>
                  <Markdown text={m.text} />
                  <MessageActions
                    role="assistant"
                    message={m}
                    isLatest={m === latestReply}
                    running={streaming || showInterrupted || pendingUser !== null}
                    onRetry={hasTools ? undefined : onRetryReply}
                    onRetryWithModel={hasTools ? undefined : onRetryWithModel}
                  />
                </div>
              )}
              <Trail entries={entriesOf(m)} />
            </Fragment>
          ),
        )}
        {pendingUser !== null && (
          <div className="me">
            {pendingUser}
            {notSent && <span className="not-sent"> Not sent.</span>}
          </div>
        )}
        <Trail entries={liveOnly} />
        <div className="reply live" aria-live="polite">
          {streaming && streamingText && <Markdown text={streamingText} />}
          {streaming && waiting !== null ? (
            <p className="waiting">Waiting {waiting} s for the model…</p>
          ) : (
            streaming && !streamingText && !anyRunning && <p className="waiting">Waiting for the model…</p>
          )}
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
