import { useEffect, useMemo, useState, type KeyboardEvent, type RefObject } from "react";
import { APP_NAME } from "../../app/brand";
import type { ComposerControl } from "../chat/Composer";
import { matchRows, type Row } from "../menu/match";
import PopMenu, { handleMenuKey, optionId, type PickMode } from "../menu/PopMenu";
import { commands, type CommandContext } from "./registry";

interface Options {
  /** Skills the menu may offer (see `useSkills`). */
  skills: Row[];
  ctx: CommandContext;
  control: RefObject<ComposerControl | null>;
}

const GROUPS = ["Commands", "Skills"];
const PREFIX = "slash";
const START = "Start.";

const commandRows: Row[] = commands.map((c) => ({
  id: c.name,
  name: c.name,
  description: c.description,
  source: APP_NAME,
  group: "Commands",
}));

/** End of the first word of `text` (its first whitespace, or its end). */
function firstWordEnd(text: string): number {
  const i = text.search(/\s/);
  return i === -1 ? text.length : i;
}

/**
 * The `/` menu: open while the box starts with "/" and the caret is in its
 * first word. Picking a command runs it; picking a skill activates it and
 * sends the rest of the box (or "Start."). With no match the box behaves
 * normally, so Enter sends the text.
 */
export function useSlashMenu({ skills, ctx, control }: Options) {
  const [box, setBox] = useState({ text: "", caret: 0 });
  const [closedFor, setClosedFor] = useState<string | null>(null);
  const [active, setActive] = useState(0);
  const { text, caret } = box;

  const end = firstWordEnd(text);
  const inWord = text.startsWith("/") && caret >= 1 && caret <= end;
  const query = text.slice(1, end);

  const rows = useMemo(() => {
    const matched = matchRows(query, [...commandRows, ...skills]);
    return GROUPS.flatMap((g) => matched.filter((r) => r.group === g));
  }, [query, skills]);

  useEffect(() => setActive(0), [query]);

  const open = inWord && rows.length > 0 && closedFor !== text;
  const current = Math.min(active, Math.max(rows.length - 1, 0));

  const pick = (row: Row, mode: PickMode) => {
    const box = control.current;
    if (!box) return;
    if (mode === "complete") {
      const rest = text.slice(end);
      box.setText(row.name + (/^\s/.test(rest) ? rest : ` ${rest}`), row.name.length + 1);
    } else if (row.group === "Commands") {
      box.setText("");
      commands.find((c) => c.name === row.name)?.run(ctx);
    } else {
      void box.submit(text.slice(end).trim() || START, { skill: row.id });
    }
  };

  const actions = {
    rows,
    active: current,
    onActiveChange: setActive,
    onPick: pick,
    onClose: () => setClosedFor(text),
  };

  const onKeyIntercept = (e: KeyboardEvent<HTMLTextAreaElement>): boolean => {
    if (!open || e.shiftKey || e.altKey || e.ctrlKey || e.metaKey) return false;
    return handleMenuKey(e, actions);
  };

  const menu = open ? (
    <PopMenu
      {...actions}
      active={current}
      groups={GROUPS}
      totals={{ Commands: commandRows.length, Skills: skills.length }}
      idPrefix={PREFIX}
      footer={
        <>
          <span>↑↓ move</span>
          <span>Enter run</span>
          <span>Tab complete</span>
          <span>Esc close</span>
          <span>Running a skill sends a message</span>
          <button
            type="button"
            className="pop-ft-link"
            onClick={() => {
              control.current?.setText("");
              ctx.openSkills();
            }}
          >
            Manage skills
          </button>
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
