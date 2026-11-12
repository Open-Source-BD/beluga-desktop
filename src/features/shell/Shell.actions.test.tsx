import { createEvent, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PackView, SkillView, StoredMessage, TurnResult } from "../../lib/ipc";
import { model } from "../models/fixtures";
import { useDropGuard } from "../../app/dropGuard";
import Shell from "./Shell";

function Guarded() {
  useDropGuard();
  return <Shell />;
}

const ipc = vi.hoisted(() => ({
  listModels: vi.fn(),
  listChats: vi.fn(),
  getMessages: vi.fn(),
  createChat: vi.fn(),
  renameChat: vi.fn(),
  deleteChat: vi.fn(),
  extraPacks: vi.fn(),
  sendMessage: vi.fn(),
  retryTurn: vi.fn(),
  stop: vi.fn(),
  keyStatus: vi.fn(),
  monthSpend: vi.fn(),
  chatUsage: vi.fn(),
  authStatus: vi.fn(),
  signOut: vi.fn(),
  listPacks: vi.fn(),
  chatState: vi.fn(),
  activateSkill: vi.fn(),
  deactivateSkill: vi.fn(),
  compactChat: vi.fn(),
  takeUserMessage: vi.fn(),
  rowsAfter: vi.fn(),
  chatFullness: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);

const PAID = model({ id: "test/model", name: "Paid One" });
const OTHER = model({ id: "test/other", name: "Other Model" });
const CHAT = { id: "c1", title: "Login page", model_id: "test/model", updated_at: 1 };
const EMPTY_STATE = { active_skills: [], folder: null, compacted: false };
const msg = (id: string, role: "user" | "assistant", text: string, seq: number): StoredMessage => ({
  id, role, text, attachments: [], interrupted: false, created_at: 1, kind: "message", seq,
});
const THREAD = [msg("u1", "user", "first q", 1), msg("a1", "assistant", "first a", 2), msg("u2", "user", "second q", 3), msg("a2", "assistant", "second a", 4)];
const done: TurnResult = { status: "done", reason: null, error: null };

const skill = (id: string, available = true): SkillView => ({
  id, name: id.split(":").pop()!, description: "", on: available, acts_on_computer: false,
  broken: null, body_chars: 10, available,
});
const packOf = (skills: SkillView[]): PackView => ({
  id: "pk", title: "pk", origin: "built_in", license: "MIT", repo: "", commit: "", about: "", on: true, error: null, skills,
});
const STARTERS = [
  skill("superpowers:brainstorming"),
  skill("superpowers:systematic-debugging"),
  skill("agent-skills:code-review-and-quality"),
];

const box = () => screen.findByLabelText("Message");
const openChat = async () => {
  await userEvent.click(await screen.findByText("Login page"));
  await screen.findByText("second a");
};

beforeEach(() => {
  vi.resetAllMocks();
  localStorage.clear();
  localStorage.setItem("lastModel", "test/model");
  ipc.listChats.mockResolvedValue([CHAT]);
  ipc.listModels.mockResolvedValue([PAID, OTHER]);
  ipc.getMessages.mockResolvedValue(THREAD);
  ipc.keyStatus.mockResolvedValue({ is_free_tier: false, free_daily: null, usage_usd: 0, limit_remaining_usd: 5 });
  ipc.monthSpend.mockResolvedValue(1.5);
  ipc.chatUsage.mockResolvedValue({ requests: 7, cost_usd: 0.25 });
  ipc.chatState.mockResolvedValue(EMPTY_STATE);
  ipc.listPacks.mockResolvedValue([packOf(STARTERS)]);
  ipc.chatFullness.mockResolvedValue({ percent: 12, context_length: 1000 });
  ipc.stop.mockResolvedValue(undefined);
  ipc.rowsAfter.mockResolvedValue(2);
  ipc.retryTurn.mockResolvedValue(done);
  ipc.takeUserMessage.mockResolvedValue({ text: "second q", attachments: [], removed: 2 });
});

describe("Shell retry", () => {
  it("Retry regenerates the latest reply with the chat's model", async () => {
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(screen.getByText("Uses one request")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("menuitem", { name: "Retry" }));
    await waitFor(() =>
      expect(ipc.retryTurn).toHaveBeenCalledWith("c1", "test/model", expect.any(Function), true),
    );
  });

  it("Retry with another model opens the picker; choosing a model regenerates with it", async () => {
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await userEvent.click(screen.getByRole("menuitem", { name: "Retry with another model…" }));
    expect(ipc.retryTurn).not.toHaveBeenCalled();
    await userEvent.click(await screen.findByRole("option", { name: /Other Model/ }));
    await waitFor(() =>
      expect(ipc.retryTurn).toHaveBeenCalledWith("c1", "test/other", expect.any(Function), true),
    );
    expect(localStorage.getItem("lastModel")).toBe("test/other");
  });

  it("closing the picker without choosing does not regenerate, and a later pick is an ordinary choice", async () => {
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await userEvent.click(screen.getByRole("menuitem", { name: "Retry with another model…" }));
    await userEvent.keyboard("{Escape}");
    await userEvent.click(screen.getByRole("button", { name: /Paid One/ }));
    await userEvent.click(await screen.findByRole("option", { name: /Other Model/ }));
    expect(ipc.retryTurn).not.toHaveBeenCalled();
  });
});

describe("Shell keys and habits", () => {
  it("Up arrow in the empty box edits the latest user message", async () => {
    render(<Shell />);
    await openChat();
    ipc.getMessages.mockResolvedValue(THREAD.slice(0, 2));
    await userEvent.type(await box(), "{ArrowUp}");
    await waitFor(() => expect(ipc.takeUserMessage).toHaveBeenCalledWith("c1", "u2"));
    expect(ipc.rowsAfter).not.toHaveBeenCalled();
    await waitFor(async () => expect(await box()).toHaveValue("second q"));
  });

  it("Up arrow with text in the box does not edit", async () => {
    render(<Shell />);
    await openChat();
    await userEvent.type(await box(), "abc{ArrowUp}");
    expect(ipc.takeUserMessage).not.toHaveBeenCalled();
  });

  it("Up arrow does nothing while a reply is running", async () => {
    ipc.sendMessage.mockReturnValue(new Promise(() => {}));
    render(<Shell />);
    await openChat();
    await userEvent.type(await box(), "go{Enter}");
    await screen.findByRole("button", { name: "Stop" });
    await userEvent.type(await box(), "{ArrowUp}");
    expect(ipc.takeUserMessage).not.toHaveBeenCalled();
  });

  it("the Settings send-key choice makes Enter add a line and Cmd/Ctrl+Enter send", async () => {
    localStorage.setItem("sendWithModifier", "true");
    ipc.sendMessage.mockResolvedValue(done);
    render(<Shell />);
    await openChat();
    await userEvent.type(await box(), "hi{Enter}there");
    expect(ipc.sendMessage).not.toHaveBeenCalled();
    expect(await box()).toHaveValue("hi\nthere");
    await userEvent.keyboard("{Control>}{Enter}{/Control}");
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalled());
    expect(ipc.sendMessage.mock.calls[0][1]).toBe("hi\nthere");
  });

  it("flipping the Settings switch takes effect when back in the chat", async () => {
    ipc.sendMessage.mockResolvedValue(done);
    ipc.authStatus.mockResolvedValue({ signed_in: true, storage: "keychain" });
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getByRole("button", { name: /Settings/ }));
    await userEvent.click(await screen.findByRole("switch", { name: "Send with Cmd/Ctrl+Enter" }));
    await userEvent.click(screen.getByRole("button", { name: "Back to chat" }));
    await userEvent.type(await box(), "a{Enter}b");
    expect(ipc.sendMessage).not.toHaveBeenCalled();
    expect(await box()).toHaveValue("a\nb");
  });

  it("Cmd/Ctrl+N starts a new chat", async () => {
    render(<Shell />);
    await openChat();
    fireEvent.keyDown(document.body, { key: "n", ctrlKey: true });
    expect(await screen.findByRole("heading", { name: "What are we building?" })).toBeInTheDocument();
  });

  it("restores a chat's draft, and clears it once the send is accepted", async () => {
    localStorage.setItem("draft:c1", "unfinished");
    ipc.sendMessage.mockResolvedValue(done);
    render(<Shell />);
    await openChat();
    await waitFor(async () => expect(await box()).toHaveValue("unfinished"));
    await userEvent.type(await box(), "{Enter}");
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalled());
    await waitFor(() => expect(localStorage.getItem("draft:c1")).toBeNull());
  });

  it("a queued message has left the box, so its draft is cleared", async () => {
    ipc.sendMessage.mockReturnValue(new Promise(() => {}));
    render(<Shell />);
    await openChat();
    await userEvent.type(await box(), "first{Enter}");
    await screen.findByRole("button", { name: "Stop" });
    await userEvent.type(await box(), "second{Enter}");
    await screen.findByText("Queued: second");
    await waitFor(() => expect(localStorage.getItem("draft:c1")).toBeNull());
    expect(await box()).toHaveValue("");
  });

  it("dropping files on the conversation attaches them and shows a drop outline while dragging", async () => {
    render(<Shell />);
    await openChat();
    const main = (await box()).closest(".main") as HTMLElement;
    const files = [new File(["a"], "a.txt", { type: "text/plain" }), new File(["b"], "b.txt", { type: "text/plain" })];
    fireEvent.dragOver(main, { dataTransfer: { types: ["Files"], files } });
    expect(main).toHaveClass("dropping");
    fireEvent.drop(main, { dataTransfer: { types: ["Files"], files } });
    expect(main).not.toHaveClass("dropping");
    const chips = await screen.findByLabelText("Attachments");
    await waitFor(() => {
      expect(within(chips).getByText("a.txt")).toBeInTheDocument();
      expect(within(chips).getByText("b.txt")).toBeInTheDocument();
    });
  });

  it("dragging something that is not a file shows no outline", async () => {
    render(<Shell />);
    await openChat();
    const main = (await box()).closest(".main") as HTMLElement;
    fireEvent.dragOver(main, { dataTransfer: { types: ["text/plain"], files: [] } });
    expect(main).not.toHaveClass("dropping");
  });
});

describe("Shell starter actions", () => {
  const start = async () => {
    render(<Shell />);
    return screen.findByRole("button", { name: /Plan a feature before writing it/ });
  };

  it("show the three actions with their secondary lines in a new chat", async () => {
    await start();
    expect(screen.getByText("asks you questions, then writes a step-by-step plan")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Find and fix a bug/ })).toBeInTheDocument();
    expect(screen.getByText("checks for mistakes and security problems")).toBeInTheDocument();
  });

  it.each([
    [/Plan a feature before writing it/, "/superpowers:brainstorming "],
    [/Find and fix a bug/, "/superpowers:systematic-debugging "],
    [/Review my recent changes/, "/agent-skills:code-review-and-quality "],
  ])("%s fills the box and sends nothing", async (name, text) => {
    await start();
    await userEvent.click(screen.getByRole("button", { name }));
    expect(await box()).toHaveValue(text);
    expect(await box()).toHaveFocus();
    expect(ipc.sendMessage).not.toHaveBeenCalled();
    expect(ipc.createChat).not.toHaveBeenCalled();
    expect(ipc.activateSkill).not.toHaveBeenCalled();
  });

  it("hides an action whose skill is off, broken or missing", async () => {
    ipc.listPacks.mockResolvedValue([
      packOf([skill("superpowers:brainstorming", false), skill("superpowers:systematic-debugging")]),
    ]);
    render(<Shell />);
    await screen.findByRole("button", { name: /Find and fix a bug/ });
    expect(screen.queryByRole("button", { name: /Plan a feature/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Review my recent changes/ })).toBeNull();
  });

  it("are not shown in an open chat", async () => {
    render(<Shell />);
    await openChat();
    expect(screen.queryByRole("button", { name: /Find and fix a bug/ })).toBeNull();
  });
});

/** Row actions show while focus is inside the row. */
const deleteFromSidebar = async (title: string) => {
  const row = screen.getAllByRole("button", { name: title }).find((b) => b.classList.contains("row-main"));
  await userEvent.click(row as HTMLElement);
  await userEvent.click(await screen.findByRole("button", { name: `Delete ${title}` }));
  await userEvent.click(await screen.findByRole("button", { name: "Delete chat" }));
};

describe("Shell deleting a chat", () => {
  const OTHER_CHAT = { id: "c2", title: "Other chat", model_id: "test/model", updated_at: 0 };

  it("removes the draft of a chat that is not selected", async () => {
    ipc.listChats.mockResolvedValue([CHAT, OTHER_CHAT]);
    ipc.deleteChat.mockResolvedValue(undefined);
    localStorage.setItem("draft:c2", "old words");
    render(<Shell />);
    await openChat();
    await deleteFromSidebar("Other chat");
    await waitFor(() => expect(ipc.deleteChat).toHaveBeenCalledWith("c2"));
    await waitFor(() => expect(localStorage.getItem("draft:c2")).toBeNull());
  });

  it("removes the draft of the selected chat even with unsaved text in the box", async () => {
    ipc.deleteChat.mockResolvedValue(undefined);
    render(<Shell />);
    await openChat();
    await userEvent.type(await box(), "typed just now");
    await deleteFromSidebar("Login page");
    await screen.findByRole("heading", { name: "What are we building?" });
    await new Promise((r) => setTimeout(r, 400));
    expect(localStorage.getItem("draft:c1")).toBeNull();
  });
});

describe("Shell Cmd/Ctrl+N", () => {
  const hello = () => screen.findByRole("heading", { name: "What are we building?" });
  const press = (init: KeyboardEventInit) => fireEvent.keyDown(document.body, init);

  it("uses the layout's own N (Dvorak puts N elsewhere) and ignores another letter on the KeyN position", async () => {
    render(<Shell />);
    await openChat();
    press({ key: "b", code: "KeyN", ctrlKey: true });
    expect(screen.queryByRole("heading", { name: "What are we building?" })).toBeNull();
    press({ key: "n", code: "KeyL", metaKey: true });
    await hello();
  });

  it("falls back to the KeyN position when the layout gives a non-Latin letter", async () => {
    render(<Shell />);
    await openChat();
    press({ key: "т", code: "KeyN", ctrlKey: true });
    await hello();
  });

  it("ignores Shift, Alt, composition and a bare N", async () => {
    render(<Shell />);
    await openChat();
    press({ key: "n", ctrlKey: true, shiftKey: true });
    press({ key: "n", ctrlKey: true, altKey: true });
    press({ key: "n", ctrlKey: true, isComposing: true });
    press({ key: "n" });
    expect(screen.queryByRole("heading", { name: "What are we building?" })).toBeNull();
  });

  it("does nothing while a dialog is open", async () => {
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[0]);
    await screen.findByRole("dialog");
    press({ key: "n", ctrlKey: true });
    expect(screen.queryByRole("heading", { name: "What are we building?" })).toBeNull();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("subscribes once, not on every render", async () => {
    const add = vi.spyOn(window, "addEventListener");
    render(<Shell />);
    await openChat();
    await userEvent.type(await box(), "abc");
    expect(add.mock.calls.filter(([t]) => t === "keydown")).toHaveLength(1);
    add.mockRestore();
  });
});

describe("Shell drops outside the conversation", () => {
  const dropOn = (el: Element) => {
    const e = createEvent.drop(el, { dataTransfer: { types: ["Files"], files: [new File(["a"], "a.txt", { type: "text/plain" })] } });
    fireEvent(el, e);
    return e.defaultPrevented;
  };

  it("a drop on the sidebar is prevented and attaches nothing", async () => {
    render(<Guarded />);
    await openChat();
    expect(dropOn(document.querySelector(".side") as Element)).toBe(true);
    expect(screen.queryByLabelText("Attachments")).toBeNull();
  });

  it("a drop on the Skills screen is prevented and attaches nothing", async () => {
    ipc.extraPacks.mockResolvedValue([]);
    render(<Guarded />);
    await openChat();
    await userEvent.click(screen.getByRole("button", { name: /Skills/ }));
    const screenEl = document.querySelector(".main") as HTMLElement;
    expect(dropOn(screenEl)).toBe(true);
    await userEvent.click(screen.getByRole("button", { name: /Back/ }));
    expect(screen.queryByLabelText("Attachments")).toBeNull();
  });

  it("a drop on the conversation while a dialog is open attaches nothing", async () => {
    render(<Guarded />);
    await openChat();
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[0]);
    await screen.findByRole("dialog");
    expect(dropOn(screen.getByRole("dialog"))).toBe(true);
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByLabelText("Attachments")).toBeNull();
  });
});
