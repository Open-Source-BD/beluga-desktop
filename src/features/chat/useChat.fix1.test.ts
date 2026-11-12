import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EngineEvent, StoredMessage, TurnResult } from "../../lib/ipc";
import { useChat } from "./useChat";

const ipc = vi.hoisted(() => ({
  getMessages: vi.fn(), sendMessage: vi.fn(), retryTurn: vi.fn(), stop: vi.fn(), chatUsage: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);

const row = (over: Partial<StoredMessage>): StoredMessage => ({
  id: "x", role: "assistant", text: "", attachments: [], interrupted: false, created_at: 1,
  kind: "message", seq: 1, tool_calls: [], tool_call_id: null, tool_meta: null, ...over,
});
const call = (id: string) => ({ id, name: "n", arguments: "{}" });
const toolRow = (id: string, callId: string): StoredMessage =>
  row({ id, role: "tool", kind: "tool", tool_call_id: callId, tool_meta: { name: "n", status: "ok", summary: "s", detail: null } });
const emits: Record<string, (e: EngineEvent) => void> = {};
const finishes: Record<string, (r: TurnResult) => void> = {};

beforeEach(() => {
  vi.resetAllMocks();
  ipc.getMessages.mockResolvedValue([]);
  ipc.chatUsage.mockResolvedValue({ requests: 0, cost_usd: 0 });
  ipc.stop.mockResolvedValue(undefined);
  ipc.sendMessage.mockImplementation((c: string, _t: string, _a: unknown, _m: string, onEvent: (e: EngineEvent) => void) => {
    emits[c] = onEvent;
    return new Promise<TurnResult>((resolve) => { finishes[c] = resolve; });
  });
});

const started = async (id = "a") => {
  const h = renderHook(({ chat }) => useChat(chat), { initialProps: { chat: id } });
  await waitFor(() => expect(ipc.getMessages).toHaveBeenCalled());
  act(() => { h.result.current.send("go", [], "m1"); });
  return h;
};
const ev = (id: string, e: EngineEvent) => act(() => emits[id](e));
const stored = async (h: Awaited<ReturnType<typeof started>>, id: string, rows: StoredMessage[], count: number) => {
  ipc.getMessages.mockResolvedValue(rows);
  ev(id, { type: "message_stored" });
  await waitFor(() => expect(h.result.current.messages).toHaveLength(count));
};
const U = row({ id: "u", role: "user", text: "go" });

describe("live calls with repeated ids", () => {
  it("the second reply's call_0 keeps its output across a message_stored and leaves only when its own result is stored", async () => {
    const h = await started();
    ev("a", { type: "tool_started", call_id: "call_0", name: "n", summary: "one" });
    ev("a", { type: "tool_finished", call_id: "call_0", status: "ok", summary: "one done", detail: null });
    const a1 = row({ id: "a1", tool_calls: [call("call_0")] });
    await stored(h, "a", [U, a1, toolRow("t1", "call_0")], 3);
    expect(h.result.current.live).toEqual([]);
    ev("a", { type: "tool_started", call_id: "call_0", name: "n", summary: "two" });
    ev("a", { type: "tool_output", call_id: "call_0", text: "out2" });
    const a2 = row({ id: "a2", tool_calls: [call("call_0")] });
    await stored(h, "a", [U, a1, toolRow("t1", "call_0"), a2], 4);
    expect(h.result.current.live).toHaveLength(1);
    expect(h.result.current.live[0]).toMatchObject({ summary: "two", output: "out2" });
    ev("a", { type: "tool_finished", call_id: "call_0", status: "ok", summary: "two done", detail: null });
    expect(h.result.current.live[0].summary).toBe("two done");
    await stored(h, "a", [U, a1, toolRow("t1", "call_0"), a2, toolRow("t2", "call_0")], 5);
    expect(h.result.current.live).toEqual([]);
  });
});

describe("streamed text meets the stored reply", () => {
  const assistant = (text: string) => [U, row({ id: "a1", text })];
  it("ignores a CRLF and trailing-space difference", async () => {
    const h = await started();
    ev("a", { type: "text_delta", text: "line one\r\nline two  " });
    await stored(h, "a", assistant("line one\nline two"), 2);
    expect(h.result.current.streamingText).toBe("");
  });
  it("a whitespace-only residue counts as empty", async () => {
    const h = await started();
    ev("a", { type: "text_delta", text: "Hello\n" });
    await stored(h, "a", assistant("Hello"), 2);
    expect(h.result.current.streamingText).toBe("");
  });
  it("text that does not match the stream is cleared: the stored row is the truth", async () => {
    const h = await started();
    ev("a", { type: "text_delta", text: "streamed words" });
    await stored(h, "a", assistant("something else"), 2);
    expect(h.result.current.streamingText).toBe("");
    ev("a", { type: "text_delta", text: "Next" });
    expect(h.result.current.streamingText).toBe("Next");
  });
  it("keeps streamed text when only a tool row arrives", async () => {
    const h = await started();
    ev("a", { type: "text_delta", text: "Thinking" });
    await stored(h, "a", [U, toolRow("t1", "c")], 2);
    expect(h.result.current.streamingText).toBe("Thinking");
  });
});

describe("a deleted chat", () => {
  it("is forgotten: its state goes and later events or results do not bring it back", async () => {
    const h = await started("a");
    ev("a", { type: "tool_started", call_id: "c", name: "n", summary: "s" });
    act(() => h.result.current.forget("a"));
    expect(h.result.current.live).toEqual([]);
    expect(h.result.current.status).toBe("idle");
    ev("a", { type: "tool_started", call_id: "d", name: "n", summary: "s" });
    ev("a", { type: "text_delta", text: "late" });
    await act(async () => finishes.a({ status: "done", reason: null, error: null }));
    expect(h.result.current.live).toEqual([]);
    expect(h.result.current.streamingText).toBe("");
    expect(h.result.current.status).toBe("idle");
  });
});

describe("round 2", () => {
  it("a user row stored mid-run does not leave a second copy of the declined call; the next reply's call_0 keeps its output", async () => {
    const h = await started();
    ev("a", { type: "tool_started", call_id: "call_0", name: "n", summary: "x" });
    ev("a", { type: "tool_finished", call_id: "call_0", status: "declined", summary: "Declined: x", detail: null });
    const a1 = row({ id: "a1", tool_calls: [call("call_0")] });
    const t1 = row({ id: "t1", role: "tool", kind: "tool", tool_call_id: "call_0", tool_meta: { name: "n", status: "declined", summary: "Declined: x", detail: null } });
    const U2 = row({ id: "u2", role: "user", text: "do y" });
    await stored(h, "a", [U, a1, t1, U2], 4);
    expect(h.result.current.live).toEqual([]);
    ev("a", { type: "tool_started", call_id: "call_0", name: "n", summary: "two" });
    ev("a", { type: "tool_output", call_id: "call_0", text: "out2" });
    await stored(h, "a", [U, a1, t1, U2, row({ id: "a2", tool_calls: [call("call_0")] })], 5);
    expect(h.result.current.live[0]).toMatchObject({ summary: "two", output: "out2" });
  });

  it("stored text with leading space still matches the stream", async () => {
    const h = await started();
    ev("a", { type: "text_delta", text: "Hello" });
    await stored(h, "a", [U, row({ id: "a1", text: "  Hello\n" })], 2);
    expect(h.result.current.streamingText).toBe("");
  });
});

describe("a send before the rows are loaded", () => {
  const OLD = [row({ id: "u0", role: "user", text: "old" }), row({ id: "a0", tool_calls: [call("call_0")] })];

  it("loads the rows first and builds the run's base from them", async () => {
    let n = 0;
    ipc.getMessages.mockImplementation(() => (++n === 1 ? new Promise(() => {}) : Promise.resolve(OLD)));
    const h = renderHook(({ chat }) => useChat(chat), { initialProps: { chat: "a" } });
    await waitFor(() => expect(ipc.getMessages).toHaveBeenCalledTimes(1));
    act(() => { h.result.current.send("go", [], "m1"); });
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalled());
    expect(h.result.current.run?.base.has("a0")).toBe(true);
    expect(h.result.current.run?.base.has("u0")).toBe(true);
  });

  it("still sends when the rows cannot be loaded; the run is then the rows after the latest user row", async () => {
    let n = 0;
    ipc.getMessages.mockImplementation(() => (++n === 1 ? new Promise(() => {}) : Promise.reject(new Error("no"))));
    const h = renderHook(({ chat }) => useChat(chat), { initialProps: { chat: "a" } });
    await waitFor(() => expect(ipc.getMessages).toHaveBeenCalledTimes(1));
    act(() => { h.result.current.send("go", [], "m1"); });
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalled());
    expect(h.result.current.status).toBe("streaming");
    expect(h.result.current.run).toBeNull();
  });
});

describe("Stop while a first send waits for the rows", () => {
  it("cancels that send: nothing is sent, the chat is idle and the text is handed back", async () => {
    let n = 0;
    let release!: (rows: StoredMessage[]) => void;
    ipc.getMessages.mockImplementation(() =>
      ++n === 1 ? new Promise(() => {}) : new Promise<StoredMessage[]>((r) => { release = r; }),
    );
    const unsent = vi.fn();
    const h = renderHook(({ chat }) => useChat(chat, undefined, unsent), { initialProps: { chat: "a" } });
    await waitFor(() => expect(ipc.getMessages).toHaveBeenCalledTimes(1));
    act(() => { h.result.current.send("my text", [], "m1"); });
    await waitFor(() => expect(ipc.getMessages).toHaveBeenCalledTimes(2));
    act(() => h.result.current.stop());
    await act(async () => release([]));
    expect(ipc.sendMessage).not.toHaveBeenCalled();
    expect(ipc.stop).not.toHaveBeenCalled();
    expect(h.result.current.status).toBe("idle");
    expect(h.result.current.pendingUser).toBeNull();
    expect(unsent).toHaveBeenCalledWith("a", "my text", []);
    // The chat can be sent to again.
    ipc.getMessages.mockResolvedValue([]);
    act(() => { h.result.current.send("again", [], "m1"); });
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalled());
  });
});
