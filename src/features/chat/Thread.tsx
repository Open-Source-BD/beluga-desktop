import { Fragment, useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { StoredMessage, UiError } from "../../lib/ipc";
import Trail from "../agent/Trail";
import { bareEntry, matchLive, pairStored, stateWord, type Run, type Slot, type TrailEntry } from "../agent/trailEntries";
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
  /** The task's own rows (not in `run.base`), so a user row mid-task does not split it. */
  run?: Run | null;
  /** The chat shown; switching it resets what the status region has read. */
  chatId?: string | null;
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
  /** Sends "continue": offered on a paused task and on an interrupted one that ran tools. */
  onContinue?: () => void;
  /** Under the Continue button of a paused task (Open Settings, cheaper models). */
  pausedExtras?: ReactNode;
  /** The core refused a retry; nothing else changed. */
  refusal?: string | null;
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
  run = null,
  chatId = null,
  waiting = null,
  notSent = false,
  onEdit,
  onRetryReply,
  onRetryWithModel,
  onRetry,
  onContinue,
  pausedExtras,
  refusal = null,
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

  // Calls are paired with their results by position, as the core does. A call with no
  // result yet takes the live line that started under it (a command still running); only a
  // call of the last reply can be running, any other unanswered one was not run.
  const stored = useMemo(() => pairStored(messages, run), [messages, run]);
  const bare = useMemo(() => {
    const out = new Map<Slot, TrailEntry>();
    for (const [id, slots] of stored.rows)
      for (const sl of slots)
        if (!sl.entry) out.set(sl, bareEntry(sl.call, streaming && id === stored.lastAssistantId));
    return out;
  }, [stored, streaming]);
  const { matched, rest } = useMemo(() => matchLive(stored, live), [stored, live]);
  const slotEntry = (sl: Slot) => sl.entry ?? matched.get(sl) ?? bare.get(sl)!;
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const toggle = useCallback(
    (key: string) =>
      setExpanded((cur) => {
        const next = new Set(cur);
        if (!next.delete(key)) next.add(key);
        return next;
      }),
    [],
  );
  const restKeys = rest.map((_, i) => `${stored.taskKey}:${stored.task.length + i}`);

  // Settled lines are announced once; running ones and their output never are.
  const announced = useRef(new Set<number>());
  const shownChat = useRef<string | null | symbol>(Symbol("none"));
  const [said, setSaid] = useState<{ id: number; text: string }[]>([]);
  useEffect(() => {
    const switched = shownChat.current !== chatId;
    shownChat.current = chatId;
    const now: { id: number; text: string }[] = [];
    for (const e of live) {
      if (e.status === "running" || e.status === "waiting" || e.liveId === undefined) continue;
      if (announced.current.has(e.liveId)) continue;
      announced.current.add(e.liveId);
      // Lines already settled when a chat is shown are not read again.
      if (switched) continue;
      const word = stateWord(e.status);
      now.push({ id: e.liveId, text: word ? `${e.summary} ${word}` : e.summary });
    }
    if (switched) setSaid([]);
    else if (now.length > 0) setSaid(now);
  }, [live, chatId]);
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
              <Trail
                entries={(stored.rows.get(m.id) ?? []).map(slotEntry)}
                keys={(stored.rows.get(m.id) ?? []).map((sl) => sl.key)}
                expanded={expanded}
                onToggle={toggle}
              />
            </Fragment>
          ),
        )}
        {pendingUser !== null && (
          <div className="me">
            {pendingUser}
            {notSent && <span className="not-sent"> Not sent.</span>}
          </div>
        )}
        <Trail entries={rest} keys={restKeys} expanded={expanded} onToggle={toggle} />
        <div role="status" className="sr-only">
          {said.map((n) => (
            <p key={n.id}>{n.text}</p>
          ))}
        </div>
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
            {hasTools && onContinue ? (
              <button type="button" className="btn" onClick={onContinue}>
                Continue
              </button>
            ) : (
              <button type="button" className="btn" onClick={onRetry}>
                Retry
              </button>
            )}
          </div>
        )}
        {status === "paused" && onContinue && (
          <div className="notice paused">
            <button type="button" className="btn" onClick={onContinue}>
              Continue
            </button>
            {pausedExtras}
          </div>
        )}
        {refusal && (
          <div className="notice">
            <p role="alert">{refusal}</p>
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
