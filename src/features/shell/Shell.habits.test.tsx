import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PackView, SkillView, StoredMessage, TurnResult } from "../../lib/ipc";
import { model } from "../models/fixtures";
import Shell from "./Shell";

const ipc = vi.hoisted(() => ({
  listModels: vi.fn(),
  listChats: vi.fn(),
  getMessages: vi.fn(),
  createChat: vi.fn(),
  renameChat: vi.fn(),
  deleteChat: vi.fn(),
  sendMessage: vi.fn(),
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
const CHAT = { id: "c1", title: "Login page", model_id: "test/model", updated_at: 1 };
const EMPTY_STATE = { active_skills: [], folder: null, compacted: false };
const msg = (id: string, role: "user" | "assistant", text: string, seq: number): StoredMessage => ({
  id, role, text, attachments: [], interrupted: false, created_at: 1, kind: "message", seq,
});
const THREAD = [msg("u1", "user", "first q", 1), msg("a1", "assistant", "first a", 2), msg("u2", "user", "second q", 3), msg("a2", "assistant", "second a", 4)];
const done: TurnResult = { status: "done", reason: null, error: null };

const skill = (id: string): SkillView => ({
  id, name: id.split(":").pop()!, description: "", on: true, acts_on_computer: false,
  broken: null, body_chars: 10, available: true,
});
const pack: PackView = {
  id: "pk", title: "pk", origin: "built_in", license: "MIT", repo: "", commit: "", about: "", on: true, error: null,
  skills: [skill("pk:skill")],
};

const box = () => screen.findByLabelText("Message");
const openChat = async () => {
  await userEvent.click(await screen.findByText("Login page"));
  await screen.findByText("second a");
};
/** Starts a reply that stays running until the returned function settles it. */
const hold = () => {
  let settle!: (r: TurnResult) => void;
  ipc.sendMessage.mockReturnValueOnce(new Promise<TurnResult>((r) => { settle = r; }));
  return (r: TurnResult) => settle(r);
};
const startReply = async (text = "go") => {
  const settle = hold();
  await userEvent.type(await box(), `${text}{Enter}`);
  await screen.findByRole("button", { name: "Stop" });
  return settle;
};

beforeEach(() => {
  vi.resetAllMocks();
  localStorage.setItem("lastModel", "test/model");
  ipc.listChats.mockResolvedValue([CHAT]);
  ipc.listModels.mockResolvedValue([PAID]);
  ipc.getMessages.mockResolvedValue(THREAD);
  ipc.keyStatus.mockResolvedValue({ is_free_tier: false, free_daily: null, usage_usd: 0, limit_remaining_usd: 5 });
  ipc.monthSpend.mockResolvedValue(1.5);
  ipc.chatUsage.mockResolvedValue({ requests: 7, cost_usd: 0.25 });
  ipc.chatState.mockResolvedValue(EMPTY_STATE);
  ipc.listPacks.mockResolvedValue([pack]);
  ipc.chatFullness.mockResolvedValue({ percent: 12, context_length: 1000 });
  ipc.stop.mockResolvedValue(undefined);
  ipc.rowsAfter.mockResolvedValue(2);
  ipc.takeUserMessage.mockResolvedValue({ text: "second q", attachments: [], removed: 2 });
});

describe("Shell edit and resend", () => {
  it("editing the latest user message acts at once: takes it, reloads, restores text and chips", async () => {
    ipc.takeUserMessage.mockResolvedValue({
      text: "second q",
      attachments: [{ name: "a.txt", mime: "text/plain", data_base64: "eA==" }],
      removed: 2,
    });
    render(<Shell />);
    await openChat();
    ipc.getMessages.mockResolvedValue(THREAD.slice(0, 2));
    const pencils = screen.getAllByRole("button", { name: "Edit message" });
    await userEvent.click(pencils[1]);
    await waitFor(() => expect(ipc.takeUserMessage).toHaveBeenCalledWith("c1", "u2"));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(await box()).toHaveValue("second q");
    expect(within(screen.getByLabelText("Attachments")).getByText("a.txt")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText("second a")).toBeNull());
    expect(screen.getByText("first a")).toBeInTheDocument();
  });

  it("an earlier message asks first, with the count; Cancel changes nothing", async () => {
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[0]);
    expect(await screen.findByText("Editing this message removes the 2 messages after it.")).toBeInTheDocument();
    expect(ipc.rowsAfter).toHaveBeenCalledWith("c1", "u1");
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(ipc.takeUserMessage).not.toHaveBeenCalled();
    expect(await box()).toHaveValue("");
  });

  it("Edit and remove takes the message", async () => {
    ipc.takeUserMessage.mockResolvedValue({ text: "first q", attachments: [], removed: 2 });
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[0]);
    await userEvent.click(await screen.findByRole("button", { name: "Edit and remove" }));
    await waitFor(() => expect(ipc.takeUserMessage).toHaveBeenCalledWith("c1", "u1"));
    await waitFor(async () => expect(await box()).toHaveValue("first q"));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("strips a leading /skill-id for an active skill from the restored text", async () => {
    ipc.chatState.mockResolvedValue({
      ...EMPTY_STATE,
      active_skills: [{ id: "pk:skill", name: "skill", too_long: false, acts_on_computer: false, unavailable: false }],
    });
    ipc.takeUserMessage.mockResolvedValue({ text: "/pk:skill do the thing", attachments: [], removed: 2 });
    render(<Shell />);
    await openChat();
    await screen.findByText("skill");
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[1]);
    await waitFor(async () => expect(await box()).toHaveValue("do the thing"));
  });

  it("a leading /skill-id that is not active stays", async () => {
    ipc.takeUserMessage.mockResolvedValue({ text: "/pk:skill do the thing", attachments: [], removed: 2 });
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[1]);
    await waitFor(async () => expect(await box()).toHaveValue("/pk:skill do the thing"));
  });

  it("hides the pencil while a reply is running", async () => {
    render(<Shell />);
    await openChat();
    expect(screen.getAllByRole("button", { name: "Edit message" })).toHaveLength(2);
    await startReply();
    expect(screen.queryByRole("button", { name: "Edit message" })).toBeNull();
  });
});

describe("Shell queued message", () => {
  it("sending during a reply queues it; a second send replaces it; attachments are not lost", async () => {
    render(<Shell />);
    await openChat();
    await startReply();
    const field = await box();
    expect(field).toBeEnabled();
    await userEvent.type(field, "x".repeat(100));
    await userEvent.type(field, "{Enter}");
    expect(screen.getByText(`Queued: ${"x".repeat(80)}`)).toBeInTheDocument();
    expect(ipc.sendMessage).toHaveBeenCalledTimes(1);
    await userEvent.type(field, "second try{Enter}");
    expect(screen.getByText("Queued: second try")).toBeInTheDocument();
    expect(screen.getAllByText(/^Queued:/)).toHaveLength(1);
  });

  it("is sent after done, through the normal path", async () => {
    render(<Shell />);
    await openChat();
    const settle = await startReply();
    await userEvent.type(await box(), "next one{Enter}");
    ipc.sendMessage.mockReturnValueOnce(new Promise(() => {}));
    settle(done);
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalledTimes(2));
    expect(ipc.sendMessage.mock.calls[1][1]).toBe("next one");
    await waitFor(() => expect(screen.queryByText(/^Queued:/)).toBeNull());
  });

  it("a queued /skill-id activates the skill like a normal send", async () => {
    render(<Shell />);
    await openChat();
    const settle = await startReply();
    await userEvent.type(await box(), "/pk:skill look{Enter}");
    ipc.sendMessage.mockReturnValueOnce(new Promise(() => {}));
    settle(done);
    await waitFor(() => expect(ipc.activateSkill).toHaveBeenCalledWith("c1", "pk:skill", "test/model"));
    await waitFor(() => expect(ipc.sendMessage.mock.calls[1][1]).toBe("look"));
  });

  it.each([
    ["failed", { status: "failed", reason: null, error: { kind: "other", message: "boom", retryable: true } } as TurnResult],
    ["interrupted", { status: "interrupted", reason: null, error: null } as TurnResult],
  ])("is kept, not sent, when the reply is %s", async (_n, result) => {
    render(<Shell />);
    await openChat();
    const settle = await startReply();
    await userEvent.type(await box(), "later{Enter}");
    settle(result);
    await screen.findByRole("button", { name: "Send" });
    expect(screen.getByText("Queued: later")).toBeInTheDocument();
    expect(ipc.sendMessage).toHaveBeenCalledTimes(1);
  });

  it("Remove queued message clears it, and it is not sent", async () => {
    render(<Shell />);
    await openChat();
    const settle = await startReply();
    await userEvent.type(await box(), "never{Enter}");
    await userEvent.click(screen.getByRole("button", { name: "Remove queued message" }));
    expect(screen.queryByText(/^Queued:/)).toBeNull();
    settle(done);
    await screen.findByRole("button", { name: "Send" });
    expect(ipc.sendMessage).toHaveBeenCalledTimes(1);
  });
});

describe("Shell Esc", () => {
  it("stops a running reply", async () => {
    render(<Shell />);
    await openChat();
    await startReply();
    await userEvent.type(await box(), "{Escape}");
    expect(ipc.stop).toHaveBeenCalledWith("c1");
  });

  it("with the slash menu open only closes the menu", async () => {
    render(<Shell />);
    await openChat();
    await startReply();
    await userEvent.type(await box(), "/");
    await screen.findByRole("listbox");
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(ipc.stop).not.toHaveBeenCalled();
    await userEvent.keyboard("{Escape}");
    expect(ipc.stop).toHaveBeenCalledWith("c1");
  });
});

describe("Shell fullness", () => {
  it("shows the percent in the header; 70 adds the /compact hint", async () => {
    ipc.chatFullness.mockResolvedValue({ percent: 70, context_length: 1000 });
    render(<Shell />);
    await openChat();
    expect(await screen.findByText("Chat is 70% full")).toBeInTheDocument();
    expect(screen.getByText("Use /compact to shorten it")).toBeInTheDocument();
  });

  it("is asked again after a turn", async () => {
    render(<Shell />);
    await openChat();
    await screen.findByText("Chat is 12% full");
    const settle = await startReply();
    const before = ipc.chatFullness.mock.calls.length;
    settle(done);
    await waitFor(() => expect(ipc.chatFullness.mock.calls.length).toBeGreaterThan(before));
  });

  it("shows nothing when the context length is unknown", async () => {
    ipc.chatFullness.mockResolvedValue({ percent: 0, context_length: 0 });
    render(<Shell />);
    await openChat();
    await waitFor(() => expect(ipc.chatFullness).toHaveBeenCalled());
    expect(screen.queryByText(/Chat is/)).toBeNull();
  });
});

describe("Shell queued message safety", () => {
  const OTHER = model({ id: "test/other", name: "Other One" });
  const CHAT_B = { id: "c2", title: "Other chat", model_id: "test/other", updated_at: 0 };

  it("a message queued in a chat you left is sent with that chat's model, to that chat", async () => {
    ipc.listModels.mockResolvedValue([PAID, OTHER]);
    ipc.listChats.mockResolvedValue([CHAT, CHAT_B]);
    render(<Shell />);
    await openChat();
    const settle = await startReply();
    await userEvent.type(await box(), "/pk:skill look{Enter}");
    await userEvent.click(await screen.findByText("Other chat"));
    ipc.sendMessage.mockReturnValueOnce(new Promise(() => {}));
    settle(done);
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalledTimes(2));
    expect(ipc.activateSkill).toHaveBeenCalledWith("c1", "pk:skill", "test/model");
    expect(ipc.sendMessage.mock.calls[1].slice(0, 2)).toEqual(["c1", "look"]);
    expect(ipc.sendMessage.mock.calls[1][3]).toBe("test/model");
  });

  it("a refused skill keeps the message queued and shows the reason with it", async () => {
    ipc.activateSkill.mockRejectedValue({ kind: "other", message: "That skill is off.", retryable: false });
    render(<Shell />);
    await openChat();
    const settle = await startReply();
    await userEvent.type(await box(), "/pk:skill look{Enter}");
    settle(done);
    expect(await screen.findByText("That skill is off.")).toBeInTheDocument();
    expect(screen.getByText("Queued: /pk:skill look")).toBeInTheDocument();
    expect(ipc.sendMessage).toHaveBeenCalledTimes(1);
  });

  it("a fresh send during the activation window does not lose or double the queued one", async () => {
    render(<Shell />);
    await openChat();
    const settle = await startReply();
    await userEvent.type(await box(), "/pk:skill look{Enter}");
    let release!: () => void;
    ipc.activateSkill.mockReturnValueOnce(new Promise<void>((r) => { release = r; }));
    let settleFresh!: (r: TurnResult) => void;
    ipc.sendMessage.mockReturnValueOnce(new Promise<TurnResult>((r) => { settleFresh = r; }));
    settle(done);
    await waitFor(() => expect(ipc.activateSkill).toHaveBeenCalledTimes(1));
    await userEvent.type(await box(), "fresh{Enter}");
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalledTimes(2));
    release();
    await waitFor(() => expect(ipc.activateSkill).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("Queued: /pk:skill look")).toBeInTheDocument();
    expect(ipc.sendMessage).toHaveBeenCalledTimes(2);
    ipc.sendMessage.mockReturnValueOnce(new Promise(() => {}));
    settleFresh(done);
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalledTimes(3));
    expect(ipc.sendMessage.mock.calls[2][1]).toBe("look");
  });

  it("deleting a chat drops its queued message; nothing is sent when its reply settles", async () => {
    ipc.deleteChat.mockResolvedValue(undefined);
    render(<Shell />);
    await openChat();
    const settle = await startReply();
    await userEvent.type(await box(), "orphan{Enter}");
    act(() => screen.getByRole("button", { name: "Login page" }).focus());
    await userEvent.click(await screen.findByRole("button", { name: "Delete Login page" }));
    ipc.listChats.mockResolvedValue([]);
    await userEvent.click(await screen.findByRole("button", { name: "Delete chat" }));
    await waitFor(() => expect(ipc.deleteChat).toHaveBeenCalledWith("c1"));
    settle(done);
    await new Promise((r) => setTimeout(r, 50));
    expect(ipc.sendMessage).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
