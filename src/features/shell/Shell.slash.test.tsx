import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PackView, SkillView, TurnResult } from "../../lib/ipc";
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
  chatFullness: vi.fn(),
  activateSkill: vi.fn(),
  deactivateSkill: vi.fn(),
  compactChat: vi.fn(),
  exportChatWithDialog: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);

const PAID = model({ id: "test/model", name: "Paid One" });
const CHAT = { id: "c1", title: "Login page", model_id: "test/model", updated_at: 1 };
const EMPTY_STATE = { active_skills: [], folder: null, compacted: false };

const skill = (id: string, over: Partial<SkillView> = {}): SkillView => ({
  id, name: id.split(":").pop()!, description: `About ${id}`, on: true, acts_on_computer: false,
  broken: null, body_chars: 10, available: true, ...over,
});
const pack = (id: string, title: string, skills: SkillView[], origin: PackView["origin"] = "built_in"): PackView => ({
  id, title, origin, license: "MIT", repo: "", commit: "", about: "", on: true, error: null, skills,
});

const box = () => screen.findByLabelText("Message");
const done: TurnResult = { status: "done", reason: null, error: null };

beforeEach(() => {
  vi.resetAllMocks();
  localStorage.setItem("lastModel", "test/model");
  ipc.listChats.mockResolvedValue([]);
  ipc.listModels.mockResolvedValue([PAID]);
  ipc.getMessages.mockResolvedValue([]);
  ipc.keyStatus.mockResolvedValue({ is_free_tier: false, free_daily: null, usage_usd: 0, limit_remaining_usd: 5 });
  ipc.monthSpend.mockResolvedValue(1.5);
  ipc.chatUsage.mockResolvedValue({ requests: 7, cost_usd: 0.25 });
  ipc.createChat.mockResolvedValue(CHAT);
  ipc.chatState.mockResolvedValue(EMPTY_STATE);
  ipc.chatFullness.mockResolvedValue({ percent: 0, context_length: 0 });
  ipc.sendMessage.mockReturnValue(new Promise(() => {}));
  ipc.listPacks.mockResolvedValue([
    pack("superpowers", "superpowers", [
      skill("superpowers:brainstorming"),
      skill("superpowers:off", { available: false, on: false }),
    ]),
    pack("yours", "Added by you", [skill("my-style")], "yours"),
  ]);
});

describe("Shell slash menu", () => {
  it("opens only when / is the first character, listing commands then available skills", async () => {
    render(<Shell />);
    const field = await box();
    await userEvent.type(field, "hello /");
    expect(screen.queryByRole("listbox")).toBeNull();
    await userEvent.clear(field);
    await userEvent.type(field, "/");
    const list = await screen.findByRole("listbox");
    expect(list).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText("/superpowers:brainstorming")).toBeInTheDocument());
    const names = screen.getAllByRole("option").map((o) => o.querySelector(".pop-name")?.textContent);
    expect(names.slice(0, 8)).toEqual(["/new", "/model", "/skills", "/usage", "/compact", "/export", "/settings", "/help"]);
    expect(names.slice(8)).toEqual(["/superpowers:brainstorming", "/my-style"]);
    expect(screen.getByText("Added by you")).toBeInTheDocument();
    expect(screen.queryByText("/superpowers:off")).toBeNull();
    expect(screen.getByText("Running a skill sends a message")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Manage skills" })).toBeInTheDocument();
  });

  it("/new starts a new chat and clears the box", async () => {
    ipc.listChats.mockResolvedValue([CHAT]);
    render(<Shell />);
    await userEvent.click(await screen.findByText("Login page"));
    await waitFor(() => expect(screen.getAllByText("Login page").length).toBeGreaterThan(1));
    await userEvent.type(await box(), "/new{Enter}");
    expect(await screen.findByText("What are we building?")).toBeInTheDocument();
    expect(await box()).toHaveValue("");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("Tab completes a row without running it", async () => {
    render(<Shell />);
    await userEvent.type(await box(), "/mo{Tab}");
    expect(await box()).toHaveValue("/model ");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("picking a skill with text activates it, then sends that text", async () => {
    const order: string[] = [];
    ipc.activateSkill.mockImplementation(async () => { order.push("activate"); return EMPTY_STATE; });
    ipc.sendMessage.mockImplementation(() => { order.push("send"); return new Promise(() => {}); });
    render(<Shell />);
    // The text is typed first; the menu follows the caret back into the first word.
    await userEvent.type(await box(), "/ a login page{Home}{ArrowRight}brain");
    await screen.findByRole("option", { name: /brainstorming/ });
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalled());
    expect(ipc.activateSkill).toHaveBeenCalledWith("c1", "superpowers:brainstorming", "test/model");
    expect(ipc.sendMessage).toHaveBeenCalledWith("c1", "a login page", [], "test/model", expect.any(Function));
    expect(order).toEqual(["activate", "send"]);
  });

  it("picking a skill with no text sends Start. and shows the chip before the reply", async () => {
    ipc.activateSkill.mockResolvedValue(EMPTY_STATE);
    ipc.chatState.mockResolvedValue({
      ...EMPTY_STATE,
      active_skills: [{ id: "my-style", name: "my-style", too_long: false, acts_on_computer: false, unavailable: false }],
    });
    render(<Shell />);
    await userEvent.type(await box(), "/my-st");
    await screen.findByRole("option", { name: /my-style/ });
    await userEvent.keyboard("{Enter}");
    await waitFor(() =>
      expect(ipc.sendMessage).toHaveBeenCalledWith("c1", "Start.", [], "test/model", expect.any(Function)),
    );
    expect(await screen.findByRole("button", { name: "Remove my-style" })).toBeInTheDocument();
    expect(await screen.findByText("Start.")).toBeInTheDocument();
  });

  it("a rejected activation shows its message and sends nothing", async () => {
    ipc.activateSkill.mockRejectedValue({ kind: "other", message: "That skill is too long for this model.", retryable: false });
    render(<Shell />);
    await userEvent.type(await box(), "/my-st");
    await screen.findByRole("option", { name: /my-style/ });
    await userEvent.keyboard("{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent("That skill is too long for this model.");
    expect(ipc.sendMessage).not.toHaveBeenCalled();
  });

  it("removing a chip deactivates the skill and refreshes", async () => {
    ipc.listChats.mockResolvedValue([CHAT]);
    const active = { id: "my-style", name: "my-style", too_long: false, acts_on_computer: false, unavailable: false };
    ipc.chatState.mockResolvedValue({ ...EMPTY_STATE, active_skills: [active] });
    ipc.deactivateSkill.mockResolvedValue(EMPTY_STATE);
    render(<Shell />);
    await userEvent.click(await screen.findByText("Login page"));
    ipc.chatState.mockResolvedValue(EMPTY_STATE);
    await userEvent.click(await screen.findByRole("button", { name: "Remove my-style" }));
    expect(ipc.deactivateSkill).toHaveBeenCalledWith("c1", "my-style");
    await waitFor(() => expect(screen.queryByRole("button", { name: "Remove my-style" })).toBeNull());
  });

  it("a tab-completed skill name followed by text activates it and sends the text", async () => {
    ipc.activateSkill.mockResolvedValue(EMPTY_STATE);
    render(<Shell />);
    await userEvent.type(await box(), "/brai{Tab}a login page{Enter}");
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalled());
    expect(ipc.activateSkill).toHaveBeenCalledWith("c1", "superpowers:brainstorming", "test/model");
    expect(ipc.sendMessage).toHaveBeenCalledWith("c1", "a login page", [], "test/model", expect.any(Function));
  });

  it("a hand-typed skill name followed by text activates it and sends the text", async () => {
    ipc.activateSkill.mockResolvedValue(EMPTY_STATE);
    render(<Shell />);
    await userEvent.type(await box(), "/my-style fix this{Enter}");
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalled());
    expect(ipc.activateSkill).toHaveBeenCalledWith("c1", "my-style", "test/model");
    expect(ipc.sendMessage).toHaveBeenCalledWith("c1", "fix this", [], "test/model", expect.any(Function));
  });

  it("a rejected activation from typed text keeps the whole text in the box", async () => {
    ipc.activateSkill.mockRejectedValue({ kind: "other", message: "Nope.", retryable: false });
    render(<Shell />);
    await userEvent.type(await box(), "/my-style fix this{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent("Nope.");
    expect(await box()).toHaveValue("/my-style fix this");
    expect(ipc.sendMessage).not.toHaveBeenCalled();
  });

  it("/compact with text passes it as the keep text, after confirmation", async () => {
    ipc.listChats.mockResolvedValue([CHAT]);
    ipc.compactChat.mockResolvedValue(done);
    render(<Shell />);
    await userEvent.click(await screen.findByText("Login page"));
    await userEvent.type(await box(), "/compact keep the database decisions{Enter}");
    expect(ipc.compactChat).not.toHaveBeenCalled();
    await userEvent.click(await screen.findByRole("button", { name: "Shorten" }));
    expect(ipc.compactChat).toHaveBeenCalledWith("c1", "test/model", "keep the database decisions");
    expect(ipc.sendMessage).not.toHaveBeenCalled();
  });

  it("a path that is not a skill or command is sent unchanged", async () => {
    render(<Shell />);
    await userEvent.type(await box(), "/usr/local/bin is on my PATH{Enter}");
    await waitFor(() =>
      expect(ipc.sendMessage).toHaveBeenCalledWith("c1", "/usr/local/bin is on my PATH", [], "test/model", expect.any(Function)),
    );
    expect(ipc.activateSkill).not.toHaveBeenCalled();
  });

  it("a command that takes no text, typed with text, is sent unchanged as a message", async () => {
    render(<Shell />);
    await userEvent.type(await box(), "/help me fix this{Enter}");
    await waitFor(() =>
      expect(ipc.sendMessage).toHaveBeenCalledWith("c1", "/help me fix this", [], "test/model", expect.any(Function)),
    );
    expect(screen.queryByRole("dialog", { name: "Help" })).toBeNull();
  });

  it("a typed command leaves the attachment chips in the box", async () => {
    render(<Shell />);
    const field = await box();
    await userEvent.upload(screen.getByLabelText("Attach files"), new File(["hi"], "a.txt", { type: "text/plain" }));
    await screen.findByText("a.txt");
    await userEvent.type(field, "/help{Enter}");
    expect(await screen.findByRole("dialog", { name: "Help" })).toBeInTheDocument();
    expect(await box()).toHaveValue("");
    expect(screen.getByText("a.txt")).toBeInTheDocument();
    expect(ipc.sendMessage).not.toHaveBeenCalled();
  });

  it("a command typed with text (not picked from the menu) leaves the chips in the box", async () => {
    render(<Shell />);
    const field = await box();
    await userEvent.upload(screen.getByLabelText("Attach files"), new File(["hi"], "a.txt", { type: "text/plain" }));
    await screen.findByText("a.txt");
    await userEvent.type(field, "/compact keep the API notes{Enter}");
    await waitFor(async () => expect(await box()).toHaveValue(""));
    expect(screen.getByText("a.txt")).toBeInTheDocument();
    expect(ipc.sendMessage).not.toHaveBeenCalled();
    expect(ipc.compactChat).not.toHaveBeenCalled();
  });

  it("an earlier alert is cleared by the next successful send", async () => {
    render(<Shell />);
    await userEvent.type(await box(), "/compact{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent("Open or start a chat first.");
    await userEvent.type(await box(), "hello{Enter}");
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalled());
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("with no match, Enter sends the literal text", async () => {
    render(<Shell />);
    await userEvent.type(await box(), "/zzzz{Enter}");
    await waitFor(() =>
      expect(ipc.sendMessage).toHaveBeenCalledWith("c1", "/zzzz", [], "test/model", expect.any(Function)),
    );
    expect(ipc.activateSkill).not.toHaveBeenCalled();
  });

  it("/help lists the commands", async () => {
    render(<Shell />);
    await userEvent.type(await box(), "/help{Enter}");
    const help = await screen.findByRole("dialog", { name: "Help" });
    expect(help).toHaveTextContent("Shorten this chat to save requests and cost");
    expect(await box()).toHaveValue("");
  });

  it("/usage shows the allowance, this chat and the month", async () => {
    ipc.listChats.mockResolvedValue([CHAT]);
    render(<Shell />);
    await userEvent.click(await screen.findByText("Login page"));
    await userEvent.type(await box(), "/usage{Enter}");
    const pop = await screen.findByRole("dialog", { name: "Usage" });
    await waitFor(() => expect(pop).toHaveTextContent("7 requests, $0.25"));
    expect(pop).toHaveTextContent("Spent this month");
    expect(pop).toHaveTextContent("$1.50");
  });

  it("/compact and /export need an open chat", async () => {
    render(<Shell />);
    await userEvent.type(await box(), "/compact{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent("Open or start a chat first.");
    await userEvent.type(await box(), "/export{Enter}");
    expect(ipc.exportChatWithDialog).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent("Open or start a chat first.");
  });

  describe("with a chat open", () => {
    beforeEach(() => {
      ipc.listChats.mockResolvedValue([CHAT]);
    });
    const open = async () => {
      render(<Shell />);
      await userEvent.click(await screen.findByText("Login page"));
    };

    it("/compact asks first; Cancel makes no request", async () => {
      await open();
      await userEvent.type(await box(), "/compact{Enter}");
      expect(await screen.findByText("Shorten this chat? This uses one request.")).toBeInTheDocument();
      await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
      expect(ipc.compactChat).not.toHaveBeenCalled();
      expect(screen.queryByText("Shorten this chat? This uses one request.")).toBeNull();
    });

    it("Shorten compacts, behaves like a running reply, then reloads the thread", async () => {
      let finish: (r: TurnResult) => void = () => {};
      ipc.compactChat.mockReturnValue(new Promise((r) => (finish = r)));
      await open();
      await userEvent.type(await box(), "/compact{Enter}");
      await userEvent.click(await screen.findByRole("button", { name: "Shorten" }));
      expect(ipc.compactChat).toHaveBeenCalledWith("c1", "test/model");
      expect(await screen.findByText("Waiting for the model…")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
      ipc.chatState.mockResolvedValue({ ...EMPTY_STATE, compacted: true });
      ipc.getMessages.mockClear();
      finish(done);
      await waitFor(() => expect(ipc.getMessages).toHaveBeenCalledWith("c1"));
      await waitFor(() => expect(screen.queryByText("Waiting for the model…")).toBeNull());
      expect(screen.queryByRole("alert")).toBeNull();
    });

    it("a done compaction that is not compacted says so", async () => {
      ipc.compactChat.mockResolvedValue(done);
      await open();
      await userEvent.type(await box(), "/compact{Enter}");
      await userEvent.click(await screen.findByRole("button", { name: "Shorten" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("The chat could not be shortened. Try again.");
    });

    it("a failed compaction shows the error's message", async () => {
      ipc.compactChat.mockResolvedValue({
        status: "failed", reason: null, error: { kind: "network", message: "No connection.", retryable: true },
      });
      await open();
      await userEvent.type(await box(), "/compact{Enter}");
      await userEvent.click(await screen.findByRole("button", { name: "Shorten" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("No connection.");
    });

    it("/export saves with the title and confirms the path; cancel shows nothing", async () => {
      ipc.exportChatWithDialog.mockResolvedValueOnce("/tmp/Login page.md").mockResolvedValueOnce(null);
      await open();
      await userEvent.type(await box(), "/export{Enter}");
      expect(ipc.exportChatWithDialog).toHaveBeenCalledWith("c1", "Login page.md");
      expect(await screen.findByText("Saved to /tmp/Login page.md")).toBeInTheDocument();
      await userEvent.type(await box(), "/export{Enter}");
      await waitFor(() => expect(ipc.exportChatWithDialog).toHaveBeenCalledTimes(2));
      await waitFor(() => expect(screen.queryByText(/Saved to/)).toBeNull());
      expect(screen.queryByRole("alert")).toBeNull();
    });
  });
});
