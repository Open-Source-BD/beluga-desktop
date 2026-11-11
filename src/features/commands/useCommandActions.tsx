import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { chatState, exportChatWithDialog, type ChatUsage, type KeyStatus } from "../../lib/ipc";
import { toUiError } from "../chat/errors";
import type { CompactOutcome, ChatStatus } from "../chat/useChat";
import Help from "./Help";
import type { CommandContext } from "./registry";
import Usage from "./Usage";
import "./commands.css";

const NO_CHAT = "Open or start a chat first.";
const NOT_SHORTENED = "The chat could not be shortened. Try again.";
const WAIT = "Wait for the reply to finish, or stop it, before shortening the chat.";
const NOTE_MS = 5000;

interface Options {
  chatId: string | null;
  title: string | undefined;
  modelId: string | null;
  status: ChatStatus;
  compact: (modelId: string, onOutcome: (o: CompactOutcome) => void) => void;
  keyStatus: KeyStatus | null;
  usage: ChatUsage | null;
  monthSpend: number;
  handlers: Pick<CommandContext, "newChat" | "openModel" | "openSkills" | "openSettings">;
}

type Panel = "usage" | "help" | "confirm" | null;

/**
 * Everything the commands do beyond navigation: the Usage and Help panels,
 * the /compact confirmation, /export, and the alert and quiet confirmation
 * line they share. `panels` goes above the message box.
 */
export function useCommandActions(o: Options): {
  ctx: CommandContext;
  panels: ReactNode;
  showAlert: (message: string) => void;
} {
  const [panel, setPanel] = useState<Panel>(null);
  const [alert, setAlert] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const latest = useRef(o);
  latest.current = o;

  // Another chat starts clean.
  useEffect(() => {
    setPanel(null);
    setAlert(null);
    setNote(null);
  }, [o.chatId]);

  useEffect(() => {
    if (note === null) return;
    const t = setTimeout(() => setNote(null), NOTE_MS);
    return () => clearTimeout(t);
  }, [note]);

  const ctx = useMemo<CommandContext>(() => {
    const begin = () => {
      setAlert(null);
      setNote(null);
    };
    const needChat = (): string | null => {
      if (latest.current.chatId) return latest.current.chatId;
      setAlert(NO_CHAT);
      return null;
    };
    return {
      newChat: () => latest.current.handlers.newChat(),
      openModel: () => latest.current.handlers.openModel(),
      openSkills: () => latest.current.handlers.openSkills(),
      openSettings: () => latest.current.handlers.openSettings(),
      showUsage: () => {
        begin();
        setPanel("usage");
      },
      showHelp: () => {
        begin();
        setPanel("help");
      },
      compact: () => {
        begin();
        const id = needChat();
        if (!id) return;
        if (latest.current.status === "streaming") setAlert(WAIT);
        else setPanel("confirm");
      },
      exportChat: () => {
        begin();
        const id = needChat();
        if (!id) return;
        exportChatWithDialog(id, `${latest.current.title ?? "Chat"}.md`).then(
          (path) => path && setNote(`Saved to ${path}`),
          (e: unknown) => setAlert(toUiError(e).message),
        );
      },
    };
  }, []);

  /** Runs the confirmed /compact and reports how it ended. */
  const runCompact = () => {
    const { chatId, modelId, compact } = latest.current;
    if (!chatId || !modelId) return;
    compact(modelId, (r) => {
      if (r.error) return setAlert(r.error.message);
      const res = r.result;
      if (res?.status === "failed") setAlert(res.error?.message ?? NOT_SHORTENED);
      else if (res?.status === "done") {
        chatState(chatId, modelId).then(
          (s) => !s.compacted && setAlert(NOT_SHORTENED),
          () => {},
        );
      }
    });
  };
  const close = () => setPanel(null);

  const panels = (
    <>
      {panel === "usage" && (
        <Usage
          onClose={close}
          freeLeft={o.keyStatus?.free_daily?.remaining ?? null}
          freeLimit={o.keyStatus?.free_daily?.limit ?? null}
          chat={o.chatId ? o.usage : null}
          monthSpend={o.monthSpend}
        />
      )}
      {panel === "help" && <Help onClose={close} />}
      {panel === "confirm" && (
        <div
          className="cmd-confirm"
          role="alertdialog"
          aria-label="Shorten this chat"
          onKeyDown={(e) => e.key === "Escape" && close()}
        >
          <p>Shorten this chat? This uses one request.</p>
          <button
            type="button"
            className="btn"
            onClick={() => {
              close();
              runCompact();
            }}
          >
            Shorten
          </button>
          <button type="button" className="btn" autoFocus onClick={close}>
            Cancel
          </button>
        </div>
      )}
      {alert && (
        <p className="cmd-alert" role="alert">
          {alert}
        </p>
      )}
      {note && (
        <p className="cmd-note" role="status">
          {note}
        </p>
      )}
    </>
  );

  return { ctx, panels, showAlert: setAlert };
}
