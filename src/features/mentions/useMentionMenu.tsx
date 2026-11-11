import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type RefObject } from "react";
import { attachFolderFile, folderFiles, type FileList } from "../../lib/ipc";
import { toUiError } from "../chat/errors";
import type { ComposerControl } from "../chat/Composer";
import type { Row } from "../menu/match";
import PopMenu, { handleMenuKey, optionId } from "../menu/PopMenu";
import { folderName, formatSize } from "./folder";

interface Options {
  chatId: string | null;
  /** The chat's folder, if it has one. */
  folder: string | null;
  /** Asks the core to open the folder dialog; true when a folder was chosen. */
  openFolder: () => Promise<boolean>;
  control: RefObject<ComposerControl | null>;
}

const PREFIX = "mention";
const DEBOUNCE_MS = 120;
const OPEN_ROW: Row = {
  id: "open-folder",
  name: "Open a folder to mention its files",
  description: "",
  source: "",
  group: "",
};

/** Start of the whitespace-free word that ends at `caret`. */
function wordStart(text: string, caret: number): number {
  let i = caret;
  while (i > 0 && !/\s/.test(text[i - 1])) i--;
  return i;
}

/** The box without the token at [start, caret): one space between what remains, caret at the join. */
function withoutToken(text: string, start: number, caret: number) {
  const left = text.slice(0, start).replace(/ +$/, "");
  const right = text.slice(caret).replace(/^ +/, "");
  const sep = left !== "" && right !== "" && !left.endsWith("\n") && !right.startsWith("\n");
  return { text: left + (sep ? " " : "") + right, caret: left.length + (sep ? 1 : 0) };
}

function fileRow(e: FileList["entries"][number]): Row {
  const slash = e.path.lastIndexOf("/");
  return {
    id: e.path,
    name: e.path.slice(slash + 1),
    description: slash === -1 ? "" : e.path.slice(0, slash + 1),
    source: e.is_dir ? "folder, lists its files" : formatSize(e.bytes),
    group: "",
  };
}

/**
 * The `@` menu: open while the caret is at the end of a whitespace-free word
 * that starts with "@". The text after the "@" is searched in the chat's
 * folder; picking a file attaches it and removes the token from the box.
 */
export function useMentionMenu({ chatId, folder, openFolder, control }: Options) {
  const [box, setBox] = useState({ text: "", caret: 0 });
  const [closedFor, setClosedFor] = useState<string | null>(null);
  const [active, setActive] = useState(0);
  const [found, setFound] = useState<FileList | null>(null);
  const latest = useRef(box);
  latest.current = box;
  const seq = useRef(0);
  const { text, caret } = box;

  const start = wordStart(text, caret);
  const inToken = text[start] === "@" && caret > start;
  const query = inToken ? text.slice(start + 1, caret) : "";
  const wanted = inToken && closedFor !== text;
  const hasFolder = chatId !== null && folder !== null;

  useEffect(() => {
    if (!wanted || !hasFolder) {
      setFound(null);
      return;
    }
    const mine = ++seq.current;
    const timer = setTimeout(() => {
      folderFiles(chatId, query).then(
        (r) => {
          if (seq.current === mine) setFound(r);
        },
        () => {
          if (seq.current === mine) setFound(null);
        },
      );
    }, DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
      seq.current++;
    };
  }, [wanted, hasFolder, chatId, query]);

  useEffect(() => setActive(0), [query, found]);
  // Closing with Esc lasts only while the text stays as it was.
  useEffect(() => {
    if (closedFor !== null && closedFor !== text) setClosedFor(null);
  }, [closedFor, text]);

  const rows = useMemo(
    () => (!hasFolder ? [OPEN_ROW] : (found?.entries ?? []).map(fileRow)),
    [hasFolder, found],
  );
  const open = wanted && rows.length > 0;
  const current = Math.min(active, rows.length - 1);
  const group = hasFolder
    ? `Files in ${folderName(folder)}${query ? ` matching "${query}"` : ""}`
    : "";
  const groups = [group];
  const shown = rows.map((r) => ({ ...r, group }));

  const pick = async (row: Row) => {
    const ctl = control.current;
    if (!ctl) return;
    if (row.id === OPEN_ROW.id) {
      const before = latest.current;
      if (await openFolder()) control.current?.setText(before.text, before.caret);
      return;
    }
    if (!chatId) return;
    try {
      const attachment = await attachFolderFile(chatId, row.id);
      ctl.addAttachment(attachment);
    } catch (e) {
      ctl.refuse(toUiError(e).message);
      return;
    }
    const now = latest.current;
    if (now.text !== text) return;
    const next = withoutToken(text, start, caret);
    ctl.setText(next.text, next.caret);
  };

  const actions = {
    rows: shown,
    active: current,
    onActiveChange: setActive,
    onPick: (row: Row) => void pick(row),
    onClose: () => setClosedFor(text),
  };

  const onKeyIntercept = (e: KeyboardEvent<HTMLTextAreaElement>): boolean => {
    if (!open || e.shiftKey || e.altKey || e.ctrlKey || e.metaKey) return false;
    return handleMenuKey(e, actions);
  };

  const menu = open ? (
    <PopMenu
      {...actions}
      groups={groups}
      totals={hasFolder ? { [group]: found?.total ?? 0 } : undefined}
      idPrefix={PREFIX}
      label="Files in the folder"
      footer={
        <>
          <span>↑↓ move</span>
          <span>Enter attach</span>
          <span>Esc close</span>
          {found?.cut && <span>list cut at 5,000 files</span>}
        </>
      }
    />
  ) : null;

  const fieldAria = open
    ? {
        role: "combobox",
        "aria-expanded": true,
        "aria-controls": `${PREFIX}-listbox`,
        "aria-activedescendant": optionId(PREFIX, current),
        "aria-autocomplete": "list" as const,
      }
    : undefined;

  return {
    menu,
    fieldAria,
    onKeyIntercept,
    onTextChange: (t: string, c: number) => setBox({ text: t, caret: c }),
  };
}
