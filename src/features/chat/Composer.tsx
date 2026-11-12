import {
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type ChangeEvent,
  type ClipboardEvent,
  type KeyboardEvent,
  type ReactNode,
  type Ref,
  type TextareaHTMLAttributes,
} from "react";
import type { AttachmentInput } from "../../lib/ipc";
import { PlusIcon, SendIcon, StopIcon } from "../shell/icons";
import {
  CANNOT_READ_IMAGES,
  MAX_ATTACHMENTS,
  MAX_ATTACHMENTS_MESSAGE,
  MAX_TEXT_BYTES,
  readAttachment,
} from "./attachments";
import { clearDraft, loadDraft, saveDraft } from "./drafts";
import { PASTE_AS_FILE_CHARS, pastedTextAttachment, pastedTextOf } from "./paste";

const DRAFT_DELAY_MS = 300;

/** Extra instructions that travel with a send, set by whoever called `submit`. */
export interface SendIntent {
  /** Activate this skill in the chat before sending. */
  skill?: string;
}

/** What the box holds: its text and attachment chips. */
export interface BoxContents {
  text: string;
  attachments: AttachmentInput[];
}

/** What menus use to edit the box and send from it; the Composer still owns the text. */
export interface ComposerControl {
  /** Replaces the text, with the caret at `caret` (default: the end). */
  setText(text: string, caret?: number): void;
  /** Sends `text` (default: the box) with the current attachments, as the Send button would. */
  submit(text?: string, intent?: SendIntent): Promise<void>;
  /** Adds a chip as a picked file would, with the same limits and refusal copy; false when refused. */
  addAttachment(attachment: AttachmentInput): boolean;
  /** Shows `message` where attachment refusals appear. */
  refuse(message: string): void;
  /** Reads and adds files as the Attach button does (dropped or pasted files), with the same limits and refusals. */
  addFiles(files: File[]): Promise<void>;
  /** Puts the caret in the box. */
  focus(): void;
  /** What the box holds now. */
  contents(): BoxContents;
  /** Replaces the text and the chips (no limits are checked), with the caret at the end. */
  replace(text: string, attachments: AttachmentInput[]): void;
}

interface Props {
  /**
   * The draft is cleared once this returns or its promise resolves; if it
   * resolves to contents, the box holds those instead of nothing. If the
   * promise rejects, the text and attachments stay in the box; showing the
   * error is the handler's job.
   */
  onSend: (
    text: string,
    attachments: AttachmentInput[],
    intent?: SendIntent,
  ) => void | BoxContents | Promise<void | BoxContents>;
  /** Told the text and caret after every edit or caret move, for menus that follow the box. */
  onTextChange?: (text: string, caret: number) => void;
  /** Sees a key first; returning true means it was handled and the box ignores it. */
  onKeyIntercept?: (e: KeyboardEvent<HTMLTextAreaElement>) => boolean;
  controlRef?: Ref<ComposerControl>;
  /**
   * Keeps the unsent text per chat: the chat's id, or null for a new chat. Left
   * out, nothing is saved or restored.
   */
  draftId?: string | null;
  /** Enter adds a new line and Cmd/Ctrl+Enter sends. */
  sendWithModifier?: boolean;
  /** Up arrow in an empty box. */
  onEmptyUp?: () => void;
  /** Esc when no reply is running (and no menu took the key). */
  onEscape?: () => void;
  /** Rendered just above the box (a popover menu), anchored to it. */
  menu?: ReactNode;
  /** Extra attributes for the text field (combobox wiring for a menu). */
  fieldAria?: TextareaHTMLAttributes<HTMLTextAreaElement>;
  onStop?: () => void;
  /** A reply is running: the button becomes Stop and Esc stops it. Sending still works (the caller queues). */
  streaming?: boolean;
  disabled?: boolean;
  /** No model chosen: the box is disabled and shows "Pick a model to start." */
  noModel?: boolean;
  /** The chat already has messages, so the placeholder reads "Reply". */
  hasMessages?: boolean;
  /** The selected model can read images; false refuses images. Unknown counts as yes. */
  acceptsImages?: boolean;
  slots?: { model?: ReactNode };
}

const MAX_HEIGHT = 200;

export default function Composer({
  onSend,
  onTextChange,
  onKeyIntercept,
  controlRef,
  draftId,
  sendWithModifier = false,
  onEmptyUp,
  onEscape,
  menu,
  fieldAria,
  onStop,
  streaming = false,
  disabled = false,
  noModel = false,
  hasMessages = false,
  acceptsImages = true,
  slots,
}: Props) {
  const [text, setText] = useState("");
  const [files, setFiles] = useState<AttachmentInput[]>([]);
  const filesRef = useRef(files);
  filesRef.current = files;
  const [refusals, setRefusals] = useState<string[]>([]);
  const [sending, setSending] = useState(false);
  const field = useRef<HTMLTextAreaElement>(null);
  const picker = useRef<HTMLInputElement>(null);
  const locked = disabled || noModel;
  const hasImage = files.some((f) => f.mime.startsWith("image/"));
  const imageBlocked = hasImage && !acceptsImages;
  const canSendText = (t: string) =>
    !locked && !sending && !imageBlocked && (t.trim() !== "" || files.length > 0);
  const canSend = canSendText(text);
  const caretAt = useRef<number | null>(null);
  const lastCaret = useRef(0);

  // Drafts: `draftKey` is the chat the text belongs to; saving waits for a pause in typing.
  const drafting = draftId !== undefined;
  const draftKey = useRef<string | null | undefined>(undefined);
  const latest = useRef(text);
  latest.current = text;
  const draftTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const persist = (id: string | null, value: string) =>
    value === "" ? clearDraft(id) : saveDraft(id, value);
  const flushDraft = () => {
    if (draftTimer.current === null) return;
    clearTimeout(draftTimer.current);
    draftTimer.current = null;
    if (draftKey.current !== undefined) persist(draftKey.current, latest.current);
  };

  const change = (next: string, caret: number) => {
    lastCaret.current = caret;
    setText(next);
    onTextChange?.(next, caret);
  };
  const notices =
    imageBlocked && !refusals.includes(CANNOT_READ_IMAGES)
      ? [...refusals, CANNOT_READ_IMAGES]
      : refusals;

  // Arriving in a chat loads its draft (after saving what was typed in the one left).
  useLayoutEffect(() => {
    if (!drafting) return;
    flushDraft();
    draftKey.current = draftId;
    const saved = loadDraft(draftId);
    caretAt.current = saved.length;
    change(saved, saved.length);
    // `change` and `flushDraft` only touch refs and state setters.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draftId, drafting]);

  useEffect(() => {
    if (!drafting || draftKey.current !== draftId) return;
    if (draftTimer.current !== null) clearTimeout(draftTimer.current);
    draftTimer.current = setTimeout(() => {
      draftTimer.current = null;
      persist(draftId, text);
    }, DRAFT_DELAY_MS);
    // A timer still waiting when the box goes away is saved by the flush below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text]);
  useEffect(() => flushDraft, []); // eslint-disable-line react-hooks/exhaustive-deps

  useLayoutEffect(() => {
    const el = field.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, MAX_HEIGHT)}px`;
  }, [text]);

  // A caret requested by `setText` is applied once the new text is in the field.
  useLayoutEffect(() => {
    if (caretAt.current !== null && field.current) {
      field.current.setSelectionRange(caretAt.current, caretAt.current);
      caretAt.current = null;
    }
  }, [text]);

  const submit = async (override?: string, intent?: SendIntent) => {
    const body = override ?? text;
    if (!canSendText(body)) return;
    setSending(true);
    let after: BoxContents | void;
    try {
      after = await (intent ? onSend(body.trim(), files, intent) : onSend(body.trim(), files));
    } catch {
      // Not accepted: keep the draft so nothing typed or attached is lost.
      return;
    } finally {
      setSending(false);
    }
    const next = after ?? { text: "", attachments: [] };
    if (draftTimer.current !== null) clearTimeout(draftTimer.current);
    draftTimer.current = null;
    if (draftKey.current !== undefined) persist(draftKey.current, next.text);
    caretAt.current = next.text.length;
    change(next.text, next.text.length);
    setFiles(next.attachments);
    setRefusals([]);
  };

  const addFiles = async (chosen: File[]) => {
    const accepted: AttachmentInput[] = [];
    const refused: string[] = [];
    const refuse = (m: string) => {
      if (!refused.includes(m)) refused.push(m);
    };
    for (const file of chosen) {
      if (files.length + accepted.length >= MAX_ATTACHMENTS) {
        refuse(MAX_ATTACHMENTS_MESSAGE);
        continue;
      }
      const r = await readAttachment(file);
      if ("error" in r) refuse(r.error);
      else if (r.attachment.mime.startsWith("image/") && !acceptsImages) refuse(CANNOT_READ_IMAGES);
      else accepted.push(r.attachment);
    }
    setFiles((cur) => [...cur, ...accepted].slice(0, MAX_ATTACHMENTS));
    setRefusals(refused);
  };

  const attach = (e: ChangeEvent<HTMLInputElement>) => {
    const chosen = Array.from(e.target.files ?? []);
    e.target.value = "";
    return addFiles(chosen);
  };

  /** Adds a chip as `addAttachment` does; false when refused (the refusal is shown). */
  const addChip = (a: AttachmentInput): boolean => {
    const refused =
      files.length >= MAX_ATTACHMENTS
        ? MAX_ATTACHMENTS_MESSAGE
        : a.mime.startsWith("image/") && !acceptsImages
          ? CANNOT_READ_IMAGES
          : null;
    if (refused) {
      setRefusals([refused]);
      return false;
    }
    setFiles((cur) => [...cur, a].slice(0, MAX_ATTACHMENTS));
    setRefusals([]);
    return true;
  };

  const onPaste = (e: ClipboardEvent<HTMLTextAreaElement>) => {
    const data = e.clipboardData;
    const images = Array.from(data.files ?? []).filter((f) => f.type.startsWith("image/"));
    if (images.length > 0) {
      e.preventDefault();
      void addFiles(images);
      return;
    }
    const pasted = data.getData("text");
    if (pasted.length < PASTE_AS_FILE_CHARS) return;
    const chip = pastedTextAttachment(pasted, files.map((f) => f.name));
    // Too big for a text attachment: let it land in the box rather than lose it.
    if (new TextEncoder().encode(pasted).length > MAX_TEXT_BYTES) return;
    e.preventDefault();
    addChip(chip);
  };

  /** Removes a pasted-text chip and puts its text in the box at the caret. */
  const putInMessage = (index: number) => {
    const chip = files[index];
    const body = chip && pastedTextOf(chip);
    const el = field.current;
    if (body === null || body === undefined || !el) return;
    const from = el.selectionStart ?? text.length;
    const to = el.selectionEnd ?? from;
    const next = text.slice(0, from) + body + text.slice(to);
    setFiles((cur) => cur.filter((_, j) => j !== index));
    caretAt.current = from + body.length;
    change(next, from + body.length);
    el.focus();
  };

  useImperativeHandle(controlRef, () => ({
    setText(next, caret = next.length) {
      caretAt.current = caret;
      change(next, caret);
    },
    submit,
    addAttachment: addChip,
    refuse: (message) => setRefusals([message]),
    addFiles,
    focus: () => field.current?.focus(),
    contents: () => ({ text: latest.current, attachments: filesRef.current }),
    replace(next, attachments) {
      caretAt.current = next.length;
      change(next, next.length);
      filesRef.current = attachments;
      setFiles(attachments);
      setRefusals([]);
    },
  }));

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (!e.nativeEvent.isComposing && onKeyIntercept?.(e)) {
      e.preventDefault();
      return;
    }
    if (e.key === "Escape" && !e.nativeEvent.isComposing && (streaming || onEscape)) {
      e.preventDefault();
      if (streaming) onStop?.();
      else onEscape?.();
      return;
    }
    if (
      e.key === "ArrowUp" &&
      text === "" &&
      onEmptyUp &&
      !e.shiftKey &&
      !e.altKey &&
      !e.ctrlKey &&
      !e.metaKey &&
      !e.nativeEvent.isComposing
    ) {
      e.preventDefault();
      onEmptyUp();
      return;
    }
    if (e.key === "Enter" && !e.nativeEvent.isComposing) {
      // Either Enter sends and Cmd/Ctrl+Enter does too, or only Cmd/Ctrl+Enter sends.
      const send = sendWithModifier ? e.metaKey || e.ctrlKey : !e.shiftKey;
      if (send) {
        e.preventDefault();
        void submit();
      }
    }
  };

  const placeholder = noModel
    ? "Pick a model to start."
    : hasMessages
      ? "Reply"
      : "Describe a feature, paste an error, or ask a question";

  return (
    <form
      className="composer"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      {menu && <div className="composer-menu">{menu}</div>}
      {files.length > 0 && (
        <ul className="attach-chips" aria-label="Attachments">
          {files.map((f, i) => (
            <li key={`${f.name}-${i}`} className="attach-chip">
              <span className="attach-name">{f.name}</span>
              {pastedTextOf(f) !== null && (
                <button type="button" className="attach-put" onClick={() => putInMessage(i)}>
                  Put in the message
                </button>
              )}
              <button
                type="button"
                className="attach-remove"
                aria-label={`Remove ${f.name}`}
                onClick={() => setFiles((cur) => cur.filter((_, j) => j !== i))}
              >
                <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" aria-hidden="true" focusable="false">
                  <path d="M6 6l12 12M18 6 6 18" />
                </svg>
              </button>
            </li>
          ))}
        </ul>
      )}
      {notices.length > 0 && (
        <div className="attach-notices" role="alert">
          {notices.map((n) => (
            <p key={n}>{n}</p>
          ))}
        </div>
      )}
      <textarea
        ref={field}
        className="composer-field"
        aria-label="Message"
        rows={1}
        value={text}
        placeholder={placeholder}
        disabled={locked}
        onChange={(e) => change(e.target.value, e.target.selectionStart)}
        onSelect={(e) => {
          const caret = e.currentTarget.selectionStart;
          if (caret !== lastCaret.current) change(e.currentTarget.value, caret);
        }}
        onKeyDown={onKeyDown}
        onPaste={onPaste}
        {...fieldAria}
      />
      <div className="composer-controls">
        <input
          ref={picker}
          type="file"
          multiple
          hidden
          tabIndex={-1}
          aria-label="Attach files"
          onChange={attach}
        />
        <button
          type="button"
          className="chip-btn"
          disabled={locked}
          onClick={() => picker.current?.click()}
        >
          <PlusIcon size={13} />
          Attach
        </button>
        <div className="composer-slot">{slots?.model}</div>
        {streaming ? (
          <button
            type="button"
            className="send"
            aria-label="Stop"
            onClick={onStop}
          >
            <StopIcon />
          </button>
        ) : (
          <button
            type="submit"
            className="send"
            aria-label="Send"
            disabled={!canSend}
          >
            <SendIcon />
          </button>
        )}
      </div>
    </form>
  );
}
