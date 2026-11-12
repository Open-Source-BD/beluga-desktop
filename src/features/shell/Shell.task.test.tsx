import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EngineEvent, StoredMessage, TurnResult } from "../../lib/ipc";
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
  peekUserMessage: vi.fn(),
  rowsAfter: vi.fn(),
  chatFullness: vi.fn(),
  taskState: vi.fn(),
  onBackgroundChanged: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);

const PAID = model({ id: "test/model", name: "Paid One" });
const A = { id: "ca", title: "Chat A", model_id: "test/model", updated_at: 2 };
const B = { id: "cb", title: "Chat B", model_id: "test/model", updated_at: 1 };
const EMPTY_STATE = { active_skills: [], folder: "/p", project: true, compacted: false };
const row = (over: Partial<StoredMessage>): StoredMessage => ({
  id: "x", role: "assistant", text: "", attachments: [], interrupted: false, created_at: 1,
  kind: "message", seq: 1, tool_calls: [], tool_call_id: null, tool_meta: null, ...over,
});
const noTask = { plan: [], edits_allowed: false, background: [], project: true, waiting: null };

const emits: Record<string, (e: EngineEvent) => void> = {};
const finishes: Record<string, (r: TurnResult) => void> = {};
const box = () => screen.findByLabelText("Message");
const open = async (title: string) => userEvent.click(await screen.findByText(title));
const send = async (text = "go") => {
  await userEvent.type(await box(), `${text}{Enter}`);
  await screen.findByRole("button", { name: "Stop" });
};
const ev = (id: string, e: EngineEvent) => act(() => emits[id](e));

beforeEach(() => {
  vi.resetAllMocks();
  localStorage.setItem("lastModel", "test/model");
  ipc.listChats.mockResolvedValue([A, B]);
  ipc.listModels.mockResolvedValue([PAID]);
  ipc.getMessages.mockResolvedValue([]);
  ipc.keyStatus.mockResolvedValue({ is_free_tier: false, free_daily: null, usage_usd: 0, limit_remaining_usd: 5 });
  ipc.monthSpend.mockResolvedValue(0);
  ipc.chatUsage.mockResolvedValue({ requests: 7, cost_usd: 0.25 });
  ipc.chatState.mockResolvedValue(EMPTY_STATE);
  ipc.listPacks.mockResolvedValue([]);
  ipc.chatFullness.mockResolvedValue({ percent: 12, context_length: 1000 });
  ipc.stop.mockResolvedValue(undefined);
  ipc.taskState.mockResolvedValue(noTask);
  ipc.onBackgroundChanged.mockResolvedValue(() => {});
  ipc.sendMessage.mockImplementation(
    (c: string, _t: string, _a: unknown, _m: string, onEvent: (e: EngineEvent) => void) => {
      emits[c] = onEvent;
      return new Promise<TurnResult>((resolve) => { finishes[c] = resolve; });
    },
  );
});

describe("Shell during a task", () => {
  it("shows the checklist above the box with states in words, and removes it when the task ends all done", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "plan_updated", items: [{ text: "Read code", state: "done" }, { text: "Write test", state: "in_progress" }] });
    const plan = await screen.findByRole("region", { name: "Task checklist" });
    expect(plan).toHaveTextContent("Read code, done");
    expect(plan).toHaveTextContent("Write test, in progress");
    expect(plan.compareDocumentPosition(await box()) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    ev("ca", { type: "plan_updated", items: [{ text: "Read code", state: "done" }, { text: "Write test", state: "done" }] });
    await act(async () => finishes.ca({ status: "done", reason: null, error: null }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Task checklist" })).toBeNull());
  });

  it("a paused task leaves the box usable, shows no error, and keeps its open checklist", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "plan_updated", items: [{ text: "Step", state: "pending" }] });
    ev("ca", { type: "paused", reason: "daily_limit" });
    ipc.getMessages.mockResolvedValue([row({ id: "n", kind: "note", role: "system", text: "Paused at the daily limit." })]);
    await act(async () => finishes.ca({ status: "paused", reason: "daily_limit", error: null }));
    expect(await screen.findByText("Paused at the daily limit.")).toBeInTheDocument();
    expect(await box()).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("region", { name: "Task checklist" })).toHaveTextContent("Step, to do");
  });

  it("shows the rate-limit wait", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "waiting", seconds: 12 });
    expect(await screen.findByText("Waiting 12 s for the model…")).toBeInTheDocument();
  });

  it("the usage line follows each usage event", async () => {
    render(<Shell />);
    await open("Chat A");
    expect(await screen.findByText("This chat: 7 requests, $0.25")).toBeInTheDocument();
    await send();
    await waitFor(() => expect(ipc.chatUsage).toHaveBeenCalled());
    ev("ca", { type: "usage", requests: 1, cost_usd: 0.1 });
    expect(await screen.findByText("This chat: 8 requests, $0.35")).toBeInTheDocument();
    ev("ca", { type: "usage", requests: 2, cost_usd: 0.3 });
    expect(await screen.findByText("This chat: 9 requests, $0.55")).toBeInTheDocument();
  });

  it("asks for the fullness again after a stored row, at most once a second", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    await waitFor(() => expect(ipc.chatFullness).toHaveBeenCalled());
    const before = ipc.chatFullness.mock.calls.length;
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      for (let i = 0; i < 5; i++) ev("ca", { type: "message_stored" });
      await act(async () => { await vi.advanceTimersByTimeAsync(1100); });
      const after = ipc.chatFullness.mock.calls.length;
      expect(after).toBeGreaterThan(before);
      expect(after - before).toBeLessThanOrEqual(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("a task running in chat A never shows in chat B, and is back when A is selected again", async () => {
    render(<Shell />);
    await open("Chat A");
    await send();
    ev("ca", { type: "tool_started", call_id: "c1", name: "read_file", summary: "Reading a-secret.ts" });
    ev("ca", { type: "plan_updated", items: [{ text: "A plan item", state: "pending" }] });
    ev("ca", { type: "text_delta", text: "A is working" });
    expect(await screen.findByText("Reading a-secret.ts")).toBeInTheDocument();
    await open("Chat B");
    expect(screen.queryByText("Reading a-secret.ts")).toBeNull();
    expect(screen.queryByText("A is working")).toBeNull();
    expect(screen.queryByText("A plan item")).toBeNull();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    ev("ca", { type: "tool_started", call_id: "c2", name: "read_file", summary: "Reading more-a.ts" });
    ev("ca", { type: "usage", requests: 1, cost_usd: 1 });
    expect(screen.queryByText("Reading more-a.ts")).toBeNull();
    expect(await screen.findByText("This chat: 7 requests, $0.25")).toBeInTheDocument();
    await open("Chat A");
    expect(await screen.findByText("Reading a-secret.ts")).toBeInTheDocument();
    expect(screen.getByText("Reading more-a.ts")).toBeInTheDocument();
    expect(screen.getByText("A plan item", { exact: false })).toBeInTheDocument();
  });

  it("a stored task shows its trail and one reply when the chat is opened", async () => {
    ipc.getMessages.mockResolvedValue([
      row({ id: "u", role: "user", text: "fix it" }),
      row({ id: "a1", tool_calls: [{ id: "c1", name: "run_command", arguments: "{}" }] }),
      row({ id: "t1", role: "tool", kind: "tool", tool_call_id: "c1", tool_meta: { name: "run_command", status: "ok", summary: "Ran `npm install` (exit 0, 14 s)", detail: "added 3 packages" } }),
      row({ id: "a2", text: "Installed." }),
    ]);
    render(<Shell />);
    await open("Chat A");
    const line = await screen.findByRole("button", { name: "Ran `npm install` (exit 0, 14 s)" });
    await userEvent.click(line);
    expect(within(document.body).getByText("added 3 packages")).toBeInTheDocument();
    expect(screen.getAllByText("Installed.")).toHaveLength(1);
  });
});
