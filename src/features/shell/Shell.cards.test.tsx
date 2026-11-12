import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Approval, EngineEvent, StoredMessage, TurnResult } from "../../lib/ipc";
import { model } from "../models/fixtures";
import { setArmDelay } from "../agent/armDelay";
import Shell from "./Shell";

const ipc = vi.hoisted(() => ({
  listModels: vi.fn(), listChats: vi.fn(), getMessages: vi.fn(), createChat: vi.fn(), renameChat: vi.fn(),
  deleteChat: vi.fn(), sendMessage: vi.fn(), retryTurn: vi.fn(), stop: vi.fn(), keyStatus: vi.fn(),
  monthSpend: vi.fn(), chatUsage: vi.fn(), authStatus: vi.fn(), signOut: vi.fn(), listPacks: vi.fn(),
  chatState: vi.fn(), activateSkill: vi.fn(), deactivateSkill: vi.fn(), compactChat: vi.fn(),
  takeUserMessage: vi.fn(), peekUserMessage: vi.fn(), rowsAfter: vi.fn(), chatFullness: vi.fn(),
  taskState: vi.fn(), onBackgroundChanged: vi.fn(), answerApproval: vi.fn(), revokeEdits: vi.fn(),
  backgroundOutput: vi.fn(), stopBackground: vi.fn(), chooseChatFolder: vi.fn(), setChatFolder: vi.fn(),
  spendLimit: vi.fn(), projectRules: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);

const PAID = model({ id: "test/model", name: "Paid One" });
const A = { id: "ca", title: "Chat A", model_id: "test/model", updated_at: 2 };
const B = { id: "cb", title: "Chat B", model_id: "test/model", updated_at: 1 };
const PROJECT_STATE = { active_skills: [], folder: "/p", project: true, compacted: false };
const row = (over: Partial<StoredMessage>): StoredMessage => ({
  id: "x", role: "assistant", text: "", attachments: [], interrupted: false, created_at: 1,
  kind: "message", seq: 1, tool_calls: [], tool_call_id: null, tool_meta: null, ...over,
});
const noTask = { plan: [], edits_allowed: false, background: [], project: true, waiting: null, running: false };
const CMD: Approval = { kind: "command", id: "ap1", command: "npm install", folder: "/p", rule: "npm install", outside: false, first_in_project: false, background: false };

const emits: Record<string, (e: EngineEvent) => void> = {};
const finishes: Record<string, (r: TurnResult) => void> = {};
const box = () => screen.findByLabelText("Message");
const open = async (title: string) => userEvent.click(await screen.findByText(title));
const send = async (text = "go") => {
  await userEvent.type(await box(), `${text}{Enter}`);
  await screen.findByRole("button", { name: "Stop" });
};
const ev = (id: string, e: EngineEvent) => act(() => emits[id](e));
const DONE: TurnResult = { status: "done", reason: null, error: null };

beforeEach(() => {
  vi.resetAllMocks();
  setArmDelay(0);
  localStorage.clear();
  localStorage.setItem("lastModel", "test/model");
  ipc.listChats.mockResolvedValue([A, B]);
  ipc.listModels.mockResolvedValue([PAID]);
  ipc.getMessages.mockResolvedValue([]);
  ipc.keyStatus.mockResolvedValue({ is_free_tier: false, free_daily: null, usage_usd: 0, limit_remaining_usd: 5 });
  ipc.monthSpend.mockResolvedValue(0);
  ipc.chatUsage.mockResolvedValue({ requests: 7, cost_usd: 0.25 });
  ipc.chatState.mockResolvedValue(PROJECT_STATE);
  ipc.listPacks.mockResolvedValue([]);
  ipc.chatFullness.mockResolvedValue({ percent: 12, context_length: 1000 });
  ipc.stop.mockResolvedValue(undefined);
  ipc.taskState.mockResolvedValue(noTask);
  ipc.onBackgroundChanged.mockResolvedValue(() => {});
  ipc.answerApproval.mockResolvedValue(undefined);
  ipc.revokeEdits.mockResolvedValue(undefined);
  ipc.backgroundOutput.mockResolvedValue("");
  ipc.authStatus.mockResolvedValue({ signed_in: true, storage: "keychain" });
  ipc.spendLimit.mockResolvedValue({ limit_usd: 1, spent_today_usd: 0 });
  ipc.projectRules.mockResolvedValue([]);
  ipc.stopBackground.mockResolvedValue(undefined);
  ipc.sendMessage.mockImplementation(
    (c: string, _t: string, _a: unknown, _m: string, onEvent: (e: EngineEvent) => void) => {
      emits[c] = onEvent;
      return new Promise<TurnResult>((resolve) => { finishes[c] = resolve; });
    },
  );
});

describe("a card in the chat", () => {
  it("appears from approval_needed above the box, with the decline button focused, and goes on approval_cleared", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "approval_needed", approval: CMD });
    const card = await screen.findByRole("group", { name: /Run this command/ });
    expect(within(card).getByText("npm install", { selector: "pre" })).toBeInTheDocument();
    expect(card.compareDocumentPosition(await box()) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await waitFor(() => expect(screen.getByRole("button", { name: "Don't run" })).toHaveFocus());
    ev("ca", { type: "approval_cleared", id: "ap1" });
    await waitFor(() => expect(screen.queryByRole("group", { name: /Run this command/ })).toBeNull());
  });

  it("Run once answers the core once; a double click sends one answer and the buttons stay disabled", async () => {
    let release!: () => void;
    ipc.answerApproval.mockReturnValue(new Promise<void>((r) => { release = r; }));
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "approval_needed", approval: CMD });
    await userEvent.dblClick(await screen.findByRole("button", { name: "Run once" }));
    expect(ipc.answerApproval).toHaveBeenCalledTimes(1);
    expect(ipc.answerApproval).toHaveBeenCalledWith("ca", "ap1", { decision: "once" });
    for (const b of within(screen.getByRole("group", { name: /Run this command/ })).getAllByRole("button")) expect(b).toBeDisabled();
    await act(async () => release());
    for (const b of within(screen.getByRole("group", { name: /Run this command/ })).getAllByRole("button")) expect(b).toBeDisabled();
  });

  it("a refusal shows as an alert near the card, refreshes the task, and re-enables the buttons", async () => {
    ipc.answerApproval.mockRejectedValue({ kind: "other", message: "That request is no longer waiting.", retryable: false });
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "approval_needed", approval: CMD });
    const before = ipc.taskState.mock.calls.length;
    await userEvent.click(await screen.findByRole("button", { name: "Run once" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("That request is no longer waiting.");
    await waitFor(() => expect(ipc.taskState.mock.calls.length).toBeGreaterThan(before));
  });

  it("a refusal for another reason keeps the card with its buttons usable again", async () => {
    ipc.answerApproval.mockRejectedValueOnce({ kind: "other", message: "Try again.", retryable: true });
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "approval_needed", approval: CMD });
    await userEvent.click(await screen.findByRole("button", { name: "Run once" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Try again.");
    expect(screen.getByRole("button", { name: "Run once" })).toBeEnabled();
  });

  it("'Allow all edits' answers all_edits and then the Edits allowed chip shows; clicking it revokes", async () => {
    const EDIT: Approval = { kind: "edit", id: "ap2", path: "a.ts", created: false, removed: ["x"], added: ["y"], removed_total: 1, added_total: 1, total_lines: 3 };
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "approval_needed", approval: EDIT });
    await userEvent.click(await screen.findByRole("button", { name: "Allow all edits in this chat" }));
    expect(ipc.answerApproval).toHaveBeenCalledWith("ca", "ap2", { decision: "all_edits" });
    ipc.taskState.mockResolvedValue({ ...noTask, edits_allowed: true });
    ev("ca", { type: "approval_cleared", id: "ap2" });
    const chip = await screen.findByRole("button", { name: "Edits allowed" });
    ipc.taskState.mockResolvedValue(noTask);
    await userEvent.click(chip);
    expect(ipc.revokeEdits).toHaveBeenCalledWith("ca");
    await waitFor(() => expect(screen.queryByRole("button", { name: "Edits allowed" })).toBeNull());
  });

  it("after a reload the core's waiting card is drawn again", async () => {
    ipc.taskState.mockResolvedValue({ ...noTask, waiting: CMD });
    render(<Shell />);
    await open("Chat A");
    expect(await screen.findByRole("group", { name: /Run this command/ })).toBeInTheDocument();
  });

  it("a card waiting in chat A is not shown in chat B, does not take focus there, and marks A in the sidebar", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    await open("Chat B");
    (await box()).focus();
    ev("ca", { type: "approval_needed", approval: CMD });
    expect(screen.queryByRole("group", { name: /Run this command/ })).toBeNull();
    expect(await box()).toHaveFocus();
    const rowA = screen.getByRole("button", { name: /Chat A/ }).closest("li") as HTMLElement;
    expect(within(rowA).getByText("needs you")).toBeInTheDocument();
    const rowB = screen.getByRole("button", { name: /Chat B/ }).closest("li") as HTMLElement;
    expect(within(rowB).queryByText("needs you")).toBeNull();
    await open("Chat A");
    expect(await screen.findByRole("group", { name: /Run this command/ })).toBeInTheDocument();
  });

  it("the card ends with the turn", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "approval_needed", approval: CMD });
    await screen.findByRole("group", { name: /Run this command/ });
    await act(async () => finishes.ca(DONE));
    await waitFor(() => expect(screen.queryByRole("group", { name: /Run this command/ })).toBeNull());
  });

  it("Esc inside the card stops the task", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "approval_needed", approval: CMD });
    await screen.findByRole("group", { name: /Run this command/ });
    await userEvent.keyboard("{Escape}");
    expect(ipc.stop).toHaveBeenCalledWith("ca");
  });
});

describe("sending with a card open", () => {
  const withCard = async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "approval_needed", approval: CMD });
    await screen.findByRole("group", { name: /Run this command/ });
  };

  it("answers the card with the message and sends nothing else; the box clears", async () => {
    await withCard();
    await userEvent.type(await box(), "use yarn instead{Enter}");
    expect(ipc.answerApproval).toHaveBeenCalledWith("ca", "ap1", { decision: "message", text: "use yarn instead" });
    expect(ipc.sendMessage).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.getByLabelText("Message")).toHaveValue(""));
  });

  it("does not move focus out of the box while typing when the card appears", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    await userEvent.type(await box(), "half");
    ev("ca", { type: "approval_needed", approval: CMD });
    await screen.findByRole("group", { name: /Run this command/ });
    expect(await box()).toHaveFocus();
    expect(screen.getByRole("button", { name: "Don't run" })).not.toHaveFocus();
  });

  it("Enter in the box never presses a card button", async () => {
    await withCard();
    await userEvent.type(await box(), "{Enter}");
    expect(ipc.answerApproval).not.toHaveBeenCalled();
  });

  it("keeps attachment chips in the box after the answer", async () => {
    await withCard();
    const file = new File(["hello"], "notes.txt", { type: "text/plain" });
    await userEvent.upload(screen.getByLabelText("Attach files"), file);
    await screen.findByText("notes.txt");
    await userEvent.type(await box(), "hi{Enter}");
    await waitFor(() => expect(ipc.answerApproval).toHaveBeenCalled());
    expect(screen.getByText("notes.txt")).toBeInTheDocument();
  });

  it("refuses an answer over 4,000 characters and keeps the text", async () => {
    await withCard();
    const field = await box();
    await userEvent.click(field);
    await userEvent.paste("y".repeat(3999));
    await userEvent.type(field, "zz");
    await userEvent.keyboard("{Enter}");
    expect(ipc.answerApproval).not.toHaveBeenCalled();
    expect(await screen.findByText("That message is too long to send as an answer. Shorten it or answer the card first.")).toBeInTheDocument();
  });

  it("a typed /command still runs as a command and does not answer the card", async () => {
    await withCard();
    await userEvent.type(await box(), "/usage{Enter}");
    expect(ipc.answerApproval).not.toHaveBeenCalled();
  });

  it("with no card a send while the task runs is queued", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    await userEvent.type(await box(), "later{Enter}");
    expect(ipc.answerApproval).not.toHaveBeenCalled();
    expect(ipc.sendMessage).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("Queued: later")).toBeInTheDocument();
  });

  it("while paused a send goes through the normal path", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "paused", reason: "keep_going_declined" });
    await act(async () => finishes.ca({ status: "paused", reason: "keep_going_declined", error: null }));
    await screen.findByRole("button", { name: "Continue" });
    await userEvent.type(await box(), "next{Enter}");
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalledTimes(2));
    expect(ipc.sendMessage.mock.calls[1][1]).toBe("next");
  });
});

describe("Continue", () => {
  it("shows on a paused task and sends 'continue'", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    await act(async () => finishes.ca({ status: "paused", reason: "keep_going_declined", error: null }));
    await userEvent.click(await screen.findByRole("button", { name: "Continue" }));
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalledTimes(2));
    expect(ipc.sendMessage.mock.calls[1][1]).toBe("continue");
  });

  it("shows on an interrupted task with tool rows instead of Retry", async () => {
    ipc.getMessages.mockResolvedValue([
      row({ id: "u", role: "user", text: "go" }),
      row({ id: "a1", tool_calls: [{ id: "c1", name: "run_command", arguments: "{}" }] }),
      row({ id: "t1", role: "tool", kind: "tool", tool_call_id: "c1", tool_meta: { name: "run_command", status: "not_run", summary: "Not run: the task was interrupted.", detail: null } }),
    ]);
    render(<Shell />);
    await open("Chat A");
    await send();
    await act(async () => finishes.ca({ status: "interrupted", reason: "stopped", error: null }));
    expect(await screen.findByRole("button", { name: "Continue" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("an interrupted plain chat keeps Retry", async () => {
    ipc.getMessages.mockResolvedValue([row({ id: "u", role: "user", text: "go" })]);
    render(<Shell />);
    await open("Chat A");
    await send();
    await act(async () => finishes.ca({ status: "interrupted", reason: "stopped", error: null }));
    expect(await screen.findByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Continue" })).toBeNull();
  });

  it("a retry the core refuses shows the message as an alert and changes nothing else", async () => {
    ipc.getMessages.mockResolvedValue([row({ id: "u", role: "user", text: "go" })]);
    ipc.retryTurn.mockRejectedValue({ kind: "other", message: "Wait for the reply to finish, or stop it, before retrying.", retryable: false });
    render(<Shell />);
    await open("Chat A");
    await send();
    await act(async () => finishes.ca({ status: "interrupted", reason: "stopped", error: null }));
    await userEvent.click(await screen.findByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Wait for the reply to finish, or stop it, before retrying.");
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(screen.getByText("Reply interrupted.")).toBeInTheDocument();
  });
});

describe("pause reasons", () => {
  const paused = async (reason: string) => {
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "paused", reason: reason as never });
    await act(async () => finishes.ca({ status: "paused", reason, error: null }));
    await screen.findByRole("button", { name: "Continue" });
  };

  it("daily_limit offers Open Settings, which opens the Settings screen", async () => {
    await paused("daily_limit");
    await userEvent.click(screen.getByRole("button", { name: "Open Settings" }));
    expect(await screen.findByRole("button", { name: "Back to chat" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Spending limit for paid models" })).toBeInTheDocument();
  });

  it("going_in_circles shows the three cheapest paid tool models; picking one sets the model and sends nothing", async () => {
    ipc.listModels.mockResolvedValue([
      PAID,
      model({ id: "t/a", name: "Cheap A", prompt_usd_per_mtok: 0.1, completion_usd_per_mtok: 0.2 }),
      model({ id: "t/b", name: "Cheap B", prompt_usd_per_mtok: 0.2, completion_usd_per_mtok: 0.2 }),
      model({ id: "t/c", name: "Cheap C", prompt_usd_per_mtok: 0.3, completion_usd_per_mtok: 0.2 }),
      model({ id: "t/d", name: "Dear D", prompt_usd_per_mtok: 9, completion_usd_per_mtok: 9 }),
      model({ id: "t/n", name: "No Tools", supports_tools: false, prompt_usd_per_mtok: 0, completion_usd_per_mtok: 0.01 }),
    ]);
    await paused("going_in_circles");
    const names = screen.getAllByRole("button").map((b) => b.textContent ?? "");
    expect(names.some((n) => n.startsWith("Cheap A"))).toBe(true);
    expect(names.some((n) => n.startsWith("Cheap C"))).toBe(true);
    expect(names.some((n) => n.startsWith("Dear D"))).toBe(false);
    expect(names.some((n) => n.startsWith("No Tools"))).toBe(false);
    const notice = screen.getByRole("button", { name: "Continue" }).closest(".notice") as HTMLElement;
    await userEvent.click(within(notice).getByRole("button", { name: /Cheap B/ }));
    expect(localStorage.getItem("lastModel")).toBe("t/b");
    expect(ipc.sendMessage).toHaveBeenCalledTimes(1);
    expect(ipc.retryTurn).not.toHaveBeenCalled();
  });
});

describe("background commands", () => {
  it("lists running commands above the box; Stop calls stopBackground; hidden when none", async () => {
    ipc.taskState.mockResolvedValue({ ...noTask, background: [{ id: "b1", command: "npm run dev", running: true, exit_code: null, started_at: 1 }] });
    render(<Shell />);
    await open("Chat A");
    expect(await screen.findByText("npm run dev")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(ipc.stopBackground).toHaveBeenCalledWith("ca", "b1");
  });

  it("shows nothing when none is running", async () => {
    render(<Shell />);
    await open("Chat A");
    await box();
    expect(screen.queryByText("Running:")).toBeNull();
  });
});

describe("project chats and the model", () => {
  it("the picker lists only tool models in a project chat", async () => {
    ipc.listModels.mockResolvedValue([PAID, model({ id: "p/plain", name: "Plain Model", supports_tools: false })]);
    ipc.taskState.mockResolvedValue(noTask);
    render(<Shell />);
    await open("Chat A");
    await userEvent.click(await screen.findByRole("button", { name: /Paid One/ }));
    expect(screen.queryByRole("option", { name: /Plain Model/ })).toBeNull();
    expect(screen.getByRole("option", { name: /Paid One/ })).toBeInTheDocument();
  });

  it("opening a folder on a model without tools shows the alert and opens the picker; nothing is switched", async () => {
    const PLAIN = model({ id: "p/plain", name: "Plain Model", supports_tools: false });
    ipc.listModels.mockResolvedValue([PLAIN, PAID]);
    localStorage.setItem("lastModel", "p/plain");
    ipc.listChats.mockResolvedValue([{ ...A, model_id: "p/plain" }]);
    ipc.chatState.mockResolvedValue({ ...PROJECT_STATE, folder: null, project: false });
    ipc.taskState.mockResolvedValue({ ...noTask, project: false });
    ipc.chooseChatFolder.mockResolvedValue("/p");
    render(<Shell />);
    await open("Chat A");
    ipc.chatState.mockResolvedValue(PROJECT_STATE);
    ipc.taskState.mockResolvedValue(noTask);
    await userEvent.click(await screen.findByRole("button", { name: /Open a folder/i }));
    expect(await screen.findByText("This model cannot use tools. Pick another model to work on the project.")).toBeInTheDocument();
    expect(await screen.findByRole("listbox", { name: "Models" })).toBeInTheDocument();
    expect(localStorage.getItem("lastModel")).toBe("p/plain");
  });
});

describe("project chat copy", () => {
  it("the empty project chat shows the project line; a free-tier key adds the top-up sentence", async () => {
    ipc.keyStatus.mockResolvedValue({ is_free_tier: true, free_daily: { used: 1, limit: 50, remaining: 49 }, usage_usd: 0, limit_remaining_usd: null });
    render(<Shell />);
    await open("Chat A");
    expect(await screen.findByText("Beluga can read files in this folder. It asks before changing a file or running a command.")).toBeInTheDocument();
    expect(await screen.findByText(/A one-time \$10 top-up on OpenRouter raises that to 1,000 a day\./)).toBeInTheDocument();
  });

  it("editing an earlier message in a project chat with tool rows after it shows the extra sentence", async () => {
    ipc.getMessages.mockResolvedValue([
      row({ id: "u1", role: "user", text: "first" }),
      row({ id: "a1", tool_calls: [{ id: "c1", name: "run_command", arguments: "{}" }] }),
      row({ id: "t1", role: "tool", kind: "tool", tool_call_id: "c1", tool_meta: { name: "run_command", status: "ok", summary: "Ran x", detail: null } }),
      row({ id: "a2", text: "Done." }),
      row({ id: "u2", role: "user", text: "second" }),
      row({ id: "a3", text: "Ok." }),
    ]);
    ipc.rowsAfter.mockResolvedValue(5);
    render(<Shell />);
    await open("Chat A");
    const first = (await screen.findByText("first")).closest(".me")!;
    await userEvent.click(within(first as HTMLElement).getByRole("button", { name: /Edit/ }));
    expect(await screen.findByText("Files and commands already changed on your computer stay as they are.")).toBeInTheDocument();
  });
});

describe("Retry on a reply", () => {
  it("is absent on a reply whose task has tool rows and present on a plain reply", async () => {
    ipc.getMessages.mockResolvedValue([
      row({ id: "u", role: "user", text: "fix it" }),
      row({ id: "a1", tool_calls: [{ id: "c1", name: "run_command", arguments: "{}" }] }),
      row({ id: "t1", role: "tool", kind: "tool", tool_call_id: "c1", tool_meta: { name: "run_command", status: "ok", summary: "Ran x", detail: null } }),
      row({ id: "a2", text: "Installed." }),
    ]);
    const { unmount } = render(<Shell />);
    await open("Chat A");
    await screen.findByText("Installed.");
    expect(screen.queryByRole("button", { name: /Retry/ })).toBeNull();
    unmount();
    ipc.getMessages.mockResolvedValue([row({ id: "u", role: "user", text: "hi" }), row({ id: "a", text: "Hello there." })]);
    render(<Shell />);
    await open("Chat A");
    await screen.findByText("Hello there.");
    expect(screen.getAllByRole("button", { name: /Retry/ }).length).toBeGreaterThan(0);
  });
});

describe("a cleared card stays cleared", () => {
  it("an older task state that still lists the card does not bring it back", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "approval_needed", approval: CMD });
    await screen.findByRole("group", { name: /Run this command/ });
    ipc.taskState.mockResolvedValue({ ...noTask, waiting: CMD });
    ev("ca", { type: "approval_cleared", id: "ap1" });
    await waitFor(() => expect(screen.queryByRole("group", { name: /Run this command/ })).toBeNull());
    await act(async () => {});
    expect(screen.queryByRole("group", { name: /Run this command/ })).toBeNull();
  });
});

const CMD2: Approval = { ...CMD, id: "ap2", command: "rm -rf build" };

describe("fix round 1: cards that follow each other", () => {
  afterEach(() => vi.useRealTimers());

  it("a second click inside 500 ms of the next card sends nothing; after it, the next card's answer", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "approval_needed", approval: CMD });
    const run = await screen.findByRole("button", { name: "Run once" });
    vi.useFakeTimers({ shouldAdvanceTime: true });
    setArmDelay(500);
    fireEvent.click(run);
    await act(async () => {});
    expect(ipc.answerApproval).toHaveBeenCalledTimes(1);
    ev("ca", { type: "approval_cleared", id: "ap1" });
    ev("ca", { type: "approval_needed", approval: CMD2 });
    expect(await screen.findByText("rm -rf build")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Run once" }));
    await act(async () => {});
    expect(ipc.answerApproval).toHaveBeenCalledTimes(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(600); });
    fireEvent.click(screen.getByRole("button", { name: "Run once" }));
    await act(async () => {});
    expect(ipc.answerApproval).toHaveBeenLastCalledWith("ca", "ap2", { decision: "once" });
  });

  it("answering chat A after going to B and back names chat A and its card", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "approval_needed", approval: CMD });
    await screen.findByRole("group", { name: /Run this command/ });
    await open("Chat B");
    await open("Chat A");
    await userEvent.click(await screen.findByRole("button", { name: "Run once" }));
    expect(ipc.answerApproval).toHaveBeenCalledWith("ca", "ap1", { decision: "once" });
  });

  it("Stop task on the card stops the task", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "approval_needed", approval: CMD });
    await userEvent.click(await screen.findByRole("button", { name: "Stop task" }));
    expect(ipc.stop).toHaveBeenCalledWith("ca");
  });

  it("a refusal that arrives after switching chats does not show in the new chat", async () => {
    let refuse!: (e: unknown) => void;
    ipc.answerApproval.mockReturnValue(new Promise((_, rej) => { refuse = rej; }));
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "approval_needed", approval: CMD });
    await userEvent.click(await screen.findByRole("button", { name: "Run once" }));
    await open("Chat B");
    await act(async () => refuse({ kind: "other", message: "Try again.", retryable: true }));
    expect(screen.queryByText("Try again.")).toBeNull();
  });

  it("no edit pencil while a card is open or the task runs", async () => {
    ipc.getMessages.mockResolvedValue([row({ id: "u", role: "user", text: "first" }), row({ id: "a", text: "Ok." })]);
    render(<Shell />);
    await open("Chat A");
    expect(await screen.findAllByRole("button", { name: "Edit message" })).toHaveLength(1);
    await send();
    ev("ca", { type: "approval_needed", approval: CMD });
    await screen.findByRole("group", { name: /Run this command/ });
    expect(screen.queryByRole("button", { name: "Edit message" })).toBeNull();
  });
});

describe("fix round 1: a task this window has no channel to", () => {
  afterEach(() => vi.useRealTimers());

  it("shows the chat as working: Stop is available, sends queue, the card is drawn, and polling ends with a reload", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    ipc.taskState.mockResolvedValue({ ...noTask, running: true });
    render(<Shell />);
    await open("Chat A");
    await userEvent.type(await box(), "later{Enter}");
    expect(await screen.findByText("Queued: later")).toBeInTheDocument();
    expect(ipc.sendMessage).not.toHaveBeenCalled();
    await userEvent.click(await screen.findByRole("button", { name: "Stop" }));
    expect(ipc.stop).toHaveBeenCalledWith("ca");
    ipc.taskState.mockResolvedValue({ ...noTask, running: true, waiting: CMD });
    const polls = ipc.taskState.mock.calls.filter((c) => c[0] === "ca").length;
    await act(async () => { await vi.advanceTimersByTimeAsync(2100); });
    expect(ipc.taskState.mock.calls.filter((c) => c[0] === "ca").length).toBeGreaterThan(polls);
    await act(async () => { await vi.advanceTimersByTimeAsync(2100); });
    expect(await screen.findByRole("group", { name: /Run this command/ })).toBeInTheDocument();
    const reads = ipc.getMessages.mock.calls.length;
    ipc.taskState.mockResolvedValue(noTask);
    await act(async () => { await vi.advanceTimersByTimeAsync(2100); });
    await waitFor(() => expect(ipc.getMessages.mock.calls.length).toBeGreaterThan(reads));
    await waitFor(() => expect(screen.queryByRole("group", { name: /Run this command/ })).toBeNull());
    const after = ipc.taskState.mock.calls.length;
    await act(async () => { await vi.advanceTimersByTimeAsync(6000); });
    expect(ipc.taskState.mock.calls.length).toBe(after);
  });

  it("a startup sweep marks other chats that wait, and a 5-second poll continues only for running chats", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    ipc.taskState.mockImplementation((id: string) =>
      Promise.resolve(id === "cb" ? { ...noTask, running: true, waiting: CMD } : noTask),
    );
    render(<Shell />);
    const rowB = (await screen.findByRole("button", { name: /Chat B/ })).closest("li") as HTMLElement;
    await waitFor(() => expect(within(rowB).getByText("needs you")).toBeInTheDocument());
    const count = (id: string) => ipc.taskState.mock.calls.filter((c) => c[0] === id).length;
    const [a0, b0] = [count("ca"), count("cb")];
    await act(async () => { await vi.advanceTimersByTimeAsync(5100); });
    expect(count("cb")).toBeGreaterThan(b0);
    expect(count("ca")).toBe(a0);
    ipc.taskState.mockResolvedValue(noTask);
    await act(async () => { await vi.advanceTimersByTimeAsync(5100); });
    await waitFor(() => expect(within(rowB).queryByText("needs you")).toBeNull());
    const b1 = count("cb");
    await act(async () => { await vi.advanceTimersByTimeAsync(11000); });
    expect(count("cb")).toBe(b1);
  });

  it("stops polling when unmounted", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    ipc.taskState.mockResolvedValue({ ...noTask, running: true });
    const { unmount } = render(<Shell />);
    await act(async () => { await vi.advanceTimersByTimeAsync(300); });
    unmount();
    const n = ipc.taskState.mock.calls.length;
    await act(async () => { await vi.advanceTimersByTimeAsync(12000); });
    expect(ipc.taskState.mock.calls.length).toBe(n);
  });
});

describe("fix round 1: a failed task that ran tools", () => {
  it("shows Continue, not Retry", async () => {
    ipc.getMessages.mockResolvedValue([
      row({ id: "u", role: "user", text: "go" }),
      row({ id: "a1", tool_calls: [{ id: "c1", name: "run_command", arguments: "{}" }] }),
      row({ id: "t1", role: "tool", kind: "tool", tool_call_id: "c1", tool_meta: { name: "run_command", status: "ok", summary: "Ran x", detail: null } }),
    ]);
    render(<Shell />);
    await open("Chat A");
    await send();
    await act(async () => finishes.ca({ status: "failed", reason: null, error: { kind: "network", message: "Offline", retryable: true } }));
    expect(await screen.findByRole("button", { name: "Continue" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });
});
