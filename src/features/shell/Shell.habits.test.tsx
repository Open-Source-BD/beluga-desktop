import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
  peekUserMessage: vi.fn(),
  rowsAfter: vi.fn(),
  retryTurn: vi.fn(),
  chatFullness: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);

const PAID = model({ id: "test/model", name: "Paid One" });
const CHAT = { id: "c1", title: "Login page", model_id: "test/model", updated_at: 1 };
const EMPTY_STATE = { active_skills: [], folder: null, compacted: false };
const msg = (id: string, role: "user" | "assistant", text: string, seq: number): StoredMessage => ({
  id, role, text, attachments: [], interrupted: false, created_at: 1, kind: "message", seq, tool_calls: [], tool_call_id: null, tool_meta: null,
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
  ipc.peekUserMessage.mockResolvedValue({ text: "second q", attachments: [] });
  ipc.activateSkill.mockResolvedValue(EMPTY_STATE);
});

describe("Shell edit and resend", () => {
  const editing = () => screen.queryByText("Editing a message");
  const cancelEdit = () => userEvent.click(within(screen.getByTestId("editing")).getByRole("button", { name: "Cancel" }));

  it("editing the latest user message acts at once: deletes nothing, restores text and chips", async () => {
    ipc.peekUserMessage.mockResolvedValue({
      text: "second q",
      attachments: [{ name: "a.txt", mime: "text/plain", data_base64: "eA==" }],
    });
    render(<Shell />);
    await openChat();
    const pencils = screen.getAllByRole("button", { name: "Edit message" });
    await userEvent.click(pencils[1]);
    await waitFor(() => expect(ipc.peekUserMessage).toHaveBeenCalledWith("c1", "u2"));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(await box()).toHaveValue("second q");
    expect(within(screen.getByLabelText("Attachments")).getByText("a.txt")).toBeInTheDocument();
    expect(editing()).toBeInTheDocument();
    expect(ipc.takeUserMessage).not.toHaveBeenCalled();
    expect(screen.getByText("second a")).toBeInTheDocument();
  });

  it("an earlier message asks first, with the count; Cancel changes nothing", async () => {
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[0]);
    expect(await screen.findByText("Editing this message removes the 2 messages after it.")).toBeInTheDocument();
    expect(ipc.rowsAfter).toHaveBeenCalledWith("c1", "u1");
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(ipc.peekUserMessage).not.toHaveBeenCalled();
    expect(ipc.takeUserMessage).not.toHaveBeenCalled();
    expect(await box()).toHaveValue("");
    expect(editing()).toBeNull();
  });

  it("Edit and remove puts the message in the box and deletes nothing yet", async () => {
    ipc.peekUserMessage.mockResolvedValue({ text: "first q", attachments: [] });
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[0]);
    await userEvent.click(await screen.findByRole("button", { name: "Edit and remove" }));
    await waitFor(() => expect(ipc.peekUserMessage).toHaveBeenCalledWith("c1", "u1"));
    await waitFor(async () => expect(await box()).toHaveValue("first q"));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(ipc.takeUserMessage).not.toHaveBeenCalled();
    expect(screen.getByText("second a")).toBeInTheDocument();
  });

  it("sending the edit removes the message and what followed, then sends the new text", async () => {
    ipc.takeUserMessage.mockResolvedValue({ text: "second q", attachments: [], removed: 1 });
    ipc.sendMessage.mockReturnValue(new Promise(() => {}));
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[1]);
    await waitFor(async () => expect(await box()).toHaveValue("second q"));
    ipc.getMessages.mockResolvedValue(THREAD.slice(0, 2));
    await userEvent.type(await box(), " again{Enter}");
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalledTimes(1));
    expect(ipc.takeUserMessage).toHaveBeenCalledWith("c1", "u2");
    expect(ipc.takeUserMessage.mock.invocationCallOrder[0]).toBeLessThan(ipc.sendMessage.mock.invocationCallOrder[0]);
    expect(ipc.sendMessage.mock.calls[0].slice(0, 2)).toEqual(["c1", "second q again"]);
    expect(editing()).toBeNull();
  });

  it("an edit sent with a /skill-id activates the skill like a normal send", async () => {
    ipc.sendMessage.mockReturnValue(new Promise(() => {}));
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[1]);
    await waitFor(async () => expect(await box()).toHaveValue("second q"));
    const field = await box();
    await userEvent.clear(field);
    await userEvent.type(field, "/pk:skill look");
    fireEvent.submit(field.closest("form")!);
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalledTimes(1));
    expect(ipc.takeUserMessage).toHaveBeenCalledWith("c1", "u2");
    expect(ipc.activateSkill).toHaveBeenCalledWith("c1", "pk:skill", "test/model");
    expect(ipc.sendMessage.mock.calls[0][1]).toBe("look");
  });

  it("Cancel puts back what the box held, keeps it as the draft and changes nothing", async () => {
    localStorage.clear();
    render(<Shell />);
    await openChat();
    await userEvent.type(await box(), "half done");
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[1]);
    await waitFor(async () => expect(await box()).toHaveValue("second q"));
    await cancelEdit();
    expect(await box()).toHaveValue("half done");
    expect(editing()).toBeNull();
    expect(localStorage.getItem("draft:c1")).toBe("half done");
    expect(ipc.takeUserMessage).not.toHaveBeenCalled();
    expect(screen.getByText("second a")).toBeInTheDocument();
  });

  it("Esc cancels the edit", async () => {
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[1]);
    await waitFor(async () => expect(await box()).toHaveValue("second q"));
    await userEvent.type(await box(), "{Escape}");
    expect(await box()).toHaveValue("");
    expect(editing()).toBeNull();
    expect(ipc.stop).not.toHaveBeenCalled();
  });

  it("after the edit is sent, the box gets back what it held before", async () => {
    ipc.sendMessage.mockReturnValue(new Promise(() => {}));
    render(<Shell />);
    await openChat();
    await userEvent.type(await box(), "half done");
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[1]);
    await waitFor(async () => expect(await box()).toHaveValue("second q"));
    await userEvent.type(await box(), "{Enter}");
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalledTimes(1));
    expect(ipc.sendMessage.mock.calls[0][1]).toBe("second q");
    await waitFor(async () => expect(await box()).toHaveValue("half done"));
  });

  it("a failed removal sends nothing, keeps the text and shows why", async () => {
    ipc.takeUserMessage.mockRejectedValue({ kind: "other", message: "That message cannot be edited.", retryable: false });
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[1]);
    await waitFor(async () => expect(await box()).toHaveValue("second q"));
    await userEvent.type(await box(), "!{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent("That message cannot be edited.");
    expect(ipc.sendMessage).not.toHaveBeenCalled();
    expect(await box()).toHaveValue("second q!");
  });

  it("a send refused after the removal keeps the text in the box as an ordinary message", async () => {
    ipc.activateSkill.mockRejectedValue({ kind: "other", message: "That skill is off.", retryable: false });
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[1]);
    await waitFor(async () => expect(await box()).toHaveValue("second q"));
    const field = await box();
    await userEvent.clear(field);
    await userEvent.type(field, "/pk:skill look");
    fireEvent.submit(field.closest("form")!);
    expect(await screen.findByRole("alert")).toHaveTextContent("That skill is off.");
    expect(ipc.takeUserMessage).toHaveBeenCalledWith("c1", "u2");
    expect(ipc.sendMessage).not.toHaveBeenCalled();
    expect(await box()).toHaveValue("/pk:skill look");
    expect(editing()).toBeNull();
  });

  it("an emptied box cannot send the edit, so nothing is removed", async () => {
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[1]);
    await waitFor(async () => expect(await box()).toHaveValue("second q"));
    await userEvent.clear(await box());
    await userEvent.type(await box(), "{Enter}");
    expect(ipc.takeUserMessage).not.toHaveBeenCalled();
    expect(ipc.sendMessage).not.toHaveBeenCalled();
    expect(editing()).toBeInTheDocument();
  });

  it("switching chats drops the edit; the chat keeps what the box held before as its draft", async () => {
    localStorage.clear();
    ipc.listChats.mockResolvedValue([CHAT, { id: "c2", title: "Other chat", model_id: "test/model", updated_at: 0 }]);
    render(<Shell />);
    await openChat();
    await userEvent.type(await box(), "half done");
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[1]);
    await waitFor(async () => expect(await box()).toHaveValue("second q"));
    await userEvent.click(screen.getByText("Other chat"));
    await waitFor(() => expect(editing()).toBeNull());
    expect(localStorage.getItem("draft:c1")).toBe("half done");
    await userEvent.click(screen.getByText("Login page"));
    await waitFor(async () => expect(await box()).toHaveValue("half done"));
    expect(editing()).toBeNull();
    expect(ipc.takeUserMessage).not.toHaveBeenCalled();
  });

  it("a reply started another way drops the edit", async () => {
    ipc.retryTurn.mockReturnValue(new Promise(() => {}));
    render(<Shell />);
    await openChat();
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[0]);
    await userEvent.click(await screen.findByRole("button", { name: "Edit and remove" }));
    await waitFor(() => expect(editing()).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await userEvent.click(screen.getByRole("menuitem", { name: "Retry" }));
    await screen.findByRole("button", { name: "Stop" });
    await waitFor(() => expect(editing()).toBeNull());
    expect(await box()).toHaveValue("");
    expect(ipc.takeUserMessage).not.toHaveBeenCalled();
  });

  it("strips a leading /skill-id for an active skill from the restored text", async () => {
    ipc.chatState.mockResolvedValue({
      ...EMPTY_STATE,
      active_skills: [{ id: "pk:skill", name: "skill", too_long: false, acts_on_computer: false, unavailable: false }],
    });
    ipc.peekUserMessage.mockResolvedValue({ text: "/pk:skill do the thing", attachments: [] });
    render(<Shell />);
    await openChat();
    await screen.findByText("skill");
    await userEvent.click(screen.getAllByRole("button", { name: "Edit message" })[1]);
    await waitFor(async () => expect(await box()).toHaveValue("do the thing"));
  });

  it("a leading /skill-id that is not active stays", async () => {
    ipc.peekUserMessage.mockResolvedValue({ text: "/pk:skill do the thing", attachments: [] });
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

describe("Shell background chat", () => {
  const CHAT_B = { id: "c2", title: "Other chat", model_id: "test/model", updated_at: 0 };
  const B_STATE = {
    active_skills: [{ id: "pk:skill", name: "skill", too_long: false, acts_on_computer: false, unavailable: false }],
    folder: "/work/app",
    compacted: false,
  };

  it("a reply ending in another chat leaves the selected chat's chips and folder alone", async () => {
    ipc.listChats.mockResolvedValue([CHAT, CHAT_B]);
    ipc.chatState.mockImplementation(async (id: string) => (id === "c2" ? B_STATE : EMPTY_STATE));
    render(<Shell />);
    await openChat();
    const settle = await startReply();
    await userEvent.click(await screen.findByText("Other chat"));
    expect(await screen.findByText("skill")).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Close folder" })).toBeInTheDocument();
    settle(done);
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.getByText("skill")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Close folder" })).toBeInTheDocument();
    await userEvent.click(screen.getByText("Login page"));
    await waitFor(() => expect(screen.queryByText("skill")).toBeNull());
    expect(await screen.findByRole("button", { name: "Open a folder" })).toBeInTheDocument();
  });

  it("a slow answer for the chat you left does not replace the chat you selected", async () => {
    ipc.listChats.mockResolvedValue([CHAT, CHAT_B]);
    let releaseA!: () => void;
    ipc.chatState.mockImplementation((id: string) =>
      id === "c1"
        ? new Promise((r) => { releaseA = () => r(EMPTY_STATE); })
        : Promise.resolve(B_STATE),
    );
    render(<Shell />);
    await openChat();
    await waitFor(() => expect(ipc.chatState).toHaveBeenCalledWith("c1", "test/model"));
    await userEvent.click(await screen.findByText("Other chat"));
    expect(await screen.findByText("skill")).toBeInTheDocument();
    await act(async () => releaseA());
    expect(screen.getByText("skill")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Close folder" })).toBeInTheDocument();
  });
});
