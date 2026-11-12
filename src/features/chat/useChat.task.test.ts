import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EngineEvent, StoredMessage, TurnResult } from "../../lib/ipc";
import { useChat } from "./useChat";

const ipc = vi.hoisted(() => ({
  getMessages: vi.fn(),
  sendMessage: vi.fn(),
  retryTurn: vi.fn(),
  stop: vi.fn(),
  chatUsage: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);

const row = (over: Partial<StoredMessage>): StoredMessage => ({
  id: "x", role: "assistant", text: "", attachments: [], interrupted: false, created_at: 1,
  kind: "message", seq: 1, tool_calls: [], tool_call_id: null, tool_meta: null, ...over,
});
const DONE: TurnResult = { status: "done", reason: null, error: null };

const emits: Record<string, (e: EngineEvent) => void> = {};
const finishes: Record<string, (r: TurnResult) => void> = {};

beforeEach(() => {
  vi.resetAllMocks();
  ipc.getMessages.mockResolvedValue([]);
  ipc.chatUsage.mockResolvedValue({ requests: 10, cost_usd: 1 });
  ipc.sendMessage.mockImplementation(
    (c: string, _t: string, _a: unknown, _m: string, onEvent: (e: EngineEvent) => void) => {
      emits[c] = onEvent;
      return new Promise<TurnResult>((resolve) => { finishes[c] = resolve; });
    },
  );
});

const started = async (id = "a") => {
  const h = renderHook(({ chat }) => useChat(chat), { initialProps: { chat: id } });
  await waitFor(() => expect(ipc.getMessages).toHaveBeenCalled());
  act(() => { h.result.current.send("go", [], "m1"); });
  return h;
};
const ev = (id: string, e: EngineEvent) => act(() => emits[id](e));

describe("useChat project work", () => {
  it("tool_started shows a running entry; output appends; finished settles it with the summary", async () => {
    const { result } = await started();
    ev("a", { type: "tool_started", call_id: "c1", name: "run_command", summary: "Running `ls`" });
    expect(result.current.live).toMatchObject([{ callId: "c1", name: "run_command", summary: "Running `ls`", status: "running" }]);
    ev("a", { type: "tool_output", call_id: "c1", text: "one\n" });
    ev("a", { type: "tool_output", call_id: "c1", text: "two\n" });
    expect(result.current.live[0].output).toBe("one\ntwo\n");
    ev("a", { type: "tool_finished", call_id: "c1", status: "ok", summary: "Ran `ls` (exit 0, 1 s)", detail: "one\ntwo" });
    expect(result.current.live[0]).toMatchObject({ status: "ok", summary: "Ran `ls` (exit 0, 1 s)", detail: "one\ntwo" });
  });

  it("keeps only the last 20,000 characters of a running line's output", async () => {
    const { result } = await started();
    ev("a", { type: "tool_started", call_id: "c1", name: "run_command", summary: "s" });
    for (let i = 0; i < 8; i++) ev("a", { type: "tool_output", call_id: "c1", text: String(i).repeat(4000) });
    const out = result.current.live[0].output!;
    expect(out).toHaveLength(20000);
    expect(out.endsWith("7".repeat(4000))).toBe(true);
  });

  it("failed, declined and not-run results keep their status", async () => {
    const { result } = await started();
    for (const [i, status] of (["error", "declined", "not_run"] as const).entries()) {
      ev("a", { type: "tool_started", call_id: `c${i}`, name: "n", summary: "s" });
      ev("a", { type: "tool_finished", call_id: `c${i}`, status, summary: "s", detail: null });
    }
    expect(result.current.live.map((e) => e.status)).toEqual(["error", "declined", "not_run"]);
  });

  it("message_stored reloads rows, keeps streaming text that is not stored, and drops live lines that are stored", async () => {
    const { result } = await started();
    ev("a", { type: "text_delta", text: "Thinking" });
    ev("a", { type: "tool_started", call_id: "c1", name: "n", summary: "s" });
    ev("a", { type: "tool_finished", call_id: "c1", status: "ok", summary: "s", detail: null });
    ev("a", { type: "tool_started", call_id: "c2", name: "n", summary: "running one" });
    ev("a", { type: "tool_output", call_id: "c2", text: "partial" });
    ipc.getMessages.mockResolvedValue([
      row({ id: "u", role: "user", text: "go" }),
      row({ id: "a1", text: "", tool_calls: [{ id: "c1", name: "n", arguments: "{}" }, { id: "c2", name: "n", arguments: "{}" }] }),
      row({ id: "t1", role: "tool", kind: "tool", tool_call_id: "c1", tool_meta: { name: "n", status: "ok", summary: "s", detail: null } }),
    ]);
    ev("a", { type: "message_stored" });
    await waitFor(() => expect(result.current.messages).toHaveLength(3));
    expect(result.current.streamingText).toBe("Thinking");
    expect(result.current.live.map((e) => e.callId)).toEqual(["c2"]);
    expect(result.current.live[0].output).toBe("partial");
    expect(result.current.pendingUser).toBeNull();
  });

  it("message_stored removes streamed text that is now a stored reply (no doubling)", async () => {
    const { result } = await started();
    ev("a", { type: "text_delta", text: "I will read it." });
    ipc.getMessages.mockResolvedValue([row({ id: "u", role: "user", text: "go" }), row({ id: "a1", text: "I will read it." })]);
    ev("a", { type: "message_stored" });
    await waitFor(() => expect(result.current.messages).toHaveLength(2));
    expect(result.current.streamingText).toBe("");
    ev("a", { type: "text_delta", text: "Next" });
    expect(result.current.streamingText).toBe("Next");
  });

  it("plan_updated keeps the checklist; waiting is shown until the model answers", async () => {
    const { result } = await started();
    ev("a", { type: "plan_updated", items: [{ text: "p", state: "in_progress" }] });
    expect(result.current.plan).toEqual([{ text: "p", state: "in_progress" }]);
    ev("a", { type: "waiting", seconds: 7 });
    expect(result.current.waiting).toBe(7);
    ev("a", { type: "text_delta", text: "x" });
    expect(result.current.waiting).toBeNull();
  });

  it("usage events give the chat's totals as they were when the task began plus the task's", async () => {
    const { result } = await started();
    await waitFor(() => expect(ipc.chatUsage).toHaveBeenCalledWith("a"));
    ev("a", { type: "usage", requests: 2, cost_usd: 0.5 });
    await waitFor(() => expect(result.current.usage).toEqual({ requests: 12, cost_usd: 1.5 }));
    ev("a", { type: "usage", requests: 3, cost_usd: 0.75 });
    expect(result.current.usage).toEqual({ requests: 13, cost_usd: 1.75 });
  });

  it("counts stored rows so the fullness meter can be asked again", async () => {
    const { result } = await started();
    const before = result.current.storedCount;
    ev("a", { type: "message_stored" });
    expect(result.current.storedCount).toBe(before + 1);
  });

  it("a paused result is a settled, non-failed state with its reason; the live lines clear", async () => {
    const { result } = await started();
    ev("a", { type: "tool_started", call_id: "c1", name: "n", summary: "s" });
    ev("a", { type: "paused", reason: "daily_limit" });
    await act(async () => finishes.a({ status: "paused", reason: "daily_limit", error: null }));
    expect(result.current.status).toBe("paused");
    expect(result.current.pausedReason).toBe("daily_limit");
    expect(result.current.error).toBeNull();
    expect(result.current.live).toEqual([]);
    expect(result.current.waiting).toBeNull();
  });

  it("a done task ends idle", async () => {
    const { result } = await started();
    await act(async () => finishes.a(DONE));
    expect(result.current.status).toBe("idle");
  });

  it("events of a task in chat A never appear in chat B, and A's state is back when selected again", async () => {
    const { result, rerender } = await started("a");
    ev("a", { type: "tool_started", call_id: "c1", name: "n", summary: "A line" });
    ev("a", { type: "plan_updated", items: [{ text: "A plan", state: "pending" }] });
    ev("a", { type: "text_delta", text: "A text" });
    rerender({ chat: "b" });
    expect(result.current.live).toEqual([]);
    expect(result.current.plan).toBeNull();
    expect(result.current.streamingText).toBe("");
    expect(result.current.status).toBe("idle");
    ev("a", { type: "tool_started", call_id: "c2", name: "n", summary: "A again" });
    expect(result.current.live).toEqual([]);
    rerender({ chat: "a" });
    expect(result.current.live.map((e) => e.summary)).toEqual(["A line", "A again"]);
    expect(result.current.status).toBe("streaming");
  });

  it("a stored reload for A that arrives while B is selected is not applied to B", async () => {
    const { result, rerender } = await started("a");
    let resolve!: (m: StoredMessage[]) => void;
    const late = new Promise<StoredMessage[]>((r) => { resolve = r; });
    ipc.getMessages.mockImplementation((id: string) => (id === "a" ? late : Promise.resolve([])));
    ev("a", { type: "message_stored" });
    rerender({ chat: "b" });
    await act(async () => resolve([row({ id: "only-a", text: "A reply" })]));
    expect(result.current.messages).toEqual([]);
  });
});
