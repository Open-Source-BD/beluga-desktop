import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EngineEvent, StoredMessage, TurnResult, UiError } from "../../lib/ipc";
import { useChat } from "./useChat";

const ipc = vi.hoisted(() => ({
  getMessages: vi.fn(),
  sendMessage: vi.fn(),
  retryTurn: vi.fn(),
  stop: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);

const msg = (id: string, role: "user" | "assistant", text: string, interrupted = false): StoredMessage => ({
  id, role, text, attachments: [], interrupted, created_at: 1, kind: "message", seq: 1, tool_calls: [], tool_call_id: null, tool_meta: null,
});

const DONE: TurnResult = { status: "done", reason: null, error: null };
const failed = (error: UiError): TurnResult => ({ status: "failed", reason: null, error });
const interrupted = (reason: string): TurnResult => ({ status: "interrupted", reason, error: null });
const OFFLINE: UiError = { kind: "network", message: "Offline", retryable: true };

let emit: (e: EngineEvent) => void;
let finish: (r: TurnResult) => void;

beforeEach(() => {
  vi.resetAllMocks();
  ipc.getMessages.mockResolvedValue([]);
  ipc.stop.mockResolvedValue(undefined);
  ipc.sendMessage.mockImplementation(
    (_c: string, _t: string, _a: unknown, _m: string, onEvent: (e: EngineEvent) => void) => {
      emit = onEvent;
      return new Promise<TurnResult>((resolve) => {
        finish = resolve;
      });
    },
  );
  ipc.retryTurn.mockImplementation(
    (_c: string, _m: string, onEvent: (e: EngineEvent) => void) => {
      emit = onEvent;
      return new Promise<TurnResult>((resolve) => {
        finish = resolve;
      });
    },
  );
});

describe("useChat", () => {
  it("accumulates text_delta into streamingText, then done moves it into messages", async () => {
    const { result } = renderHook(() => useChat("a"));
    await waitFor(() => expect(ipc.getMessages).toHaveBeenCalled());
    act(() => result.current.send("hi", [], "m1"));
    expect(result.current.status).toBe("streaming");
    expect(ipc.sendMessage).toHaveBeenCalledWith("a", "hi", [], "m1", expect.any(Function));
    act(() => emit({ type: "text_delta", text: "Hel" }));
    act(() => emit({ type: "text_delta", text: "lo" }));
    expect(result.current.streamingText).toBe("Hello");
    ipc.getMessages.mockResolvedValue([msg("1", "user", "hi"), msg("2", "assistant", "Hello")]);
    await act(async () => {
      emit({ type: "done" });
      finish(DONE);
    });
    expect(result.current.status).toBe("idle");
    expect(result.current.streamingText).toBe("");
    expect(result.current.messages.map((m) => m.text)).toEqual(["hi", "Hello"]);
  });

  it("a paused or unknown result settles as idle without an error", async () => {
    const { result } = renderHook(() => useChat("a"));
    await waitFor(() => expect(ipc.getMessages).toHaveBeenCalled());
    for (const status of ["paused", "something_new"]) {
      act(() => result.current.send("hi", [], "m1"));
      act(() => emit({ type: "paused", reason: "going_in_circles" } as unknown as EngineEvent));
      await act(async () => finish({ status, reason: "going_in_circles", error: null } as TurnResult));
      expect(result.current.status).toBe("idle");
      expect(result.current.error).toBeNull();
    }
  });

  it("failed sets the error and keeps the user's message", async () => {
    const { result } = renderHook(() => useChat("a"));
    act(() => result.current.send("hi", [], "m1"));
    ipc.getMessages.mockResolvedValue([msg("1", "user", "hi")]);
    await act(async () => finish(failed(OFFLINE)));
    expect(result.current.status).toBe("failed");
    expect(result.current.error).toEqual(OFFLINE);
    expect(result.current.messages).toHaveLength(1);
  });

  it("a rejected send becomes a failed state", async () => {
    ipc.sendMessage.mockRejectedValue({ kind: "other", message: "No such chat", retryable: false });
    const { result } = renderHook(() => useChat("a"));
    await act(async () => result.current.send("hi", [], "m1"));
    await waitFor(() => expect(result.current.status).toBe("failed"));
    expect(result.current.error?.message).toBe("No such chat");
  });

  it("interrupted keeps the partial reply; retry resends the last user message", async () => {
    const { result } = renderHook(() => useChat("a"));
    act(() => result.current.send("hi", [], "m1"));
    ipc.getMessages.mockResolvedValue([msg("1", "user", "hi"), msg("2", "assistant", "Hel", true)]);
    await act(async () => finish(interrupted("stopped")));
    expect(result.current.status).toBe("interrupted");
    expect(result.current.messages[1]).toMatchObject({ text: "Hel", interrupted: true });
    act(() => result.current.retry());
    expect(ipc.retryTurn).toHaveBeenCalledWith("a", "m1", expect.any(Function));
    expect(ipc.sendMessage).toHaveBeenCalledTimes(1);
    expect(result.current.status).toBe("streaming");
  });

  it("after a failed event (message stored) retry uses retryTurn, never sendMessage", async () => {
    const { result } = renderHook(() => useChat("a"));
    act(() => result.current.send("hi", [], "m1"));
    ipc.getMessages.mockResolvedValue([msg("1", "user", "hi")]);
    await act(async () => finish(failed(OFFLINE)));
    act(() => result.current.retry());
    expect(ipc.retryTurn).toHaveBeenCalledWith("a", "m1", expect.any(Function));
    expect(ipc.sendMessage).toHaveBeenCalledTimes(1);
    expect(result.current.error).toBeNull();
  });

  it("a rejected send in a new chat keeps the text, and retry sends that text again", async () => {
    ipc.sendMessage.mockRejectedValueOnce({ kind: "other", message: "db down", retryable: true });
    const att = { name: "n", mime: "text/plain", data_base64: "aGk=" };
    const { result } = renderHook(() => useChat("a"));
    act(() => result.current.send("typed", [att], "m1"));
    await waitFor(() => expect(result.current.status).toBe("failed"));
    expect(result.current.pendingUser).toBe("typed");
    expect(result.current.unsent).not.toBeNull();
    act(() => result.current.retry());
    expect(ipc.sendMessage).toHaveBeenLastCalledWith("a", "typed", [att], "m1", expect.any(Function));
    expect(ipc.retryTurn).not.toHaveBeenCalled();
    expect(result.current.unsent).toBeNull();
  });

  it("a rejected send in a chat with earlier messages retries the new text, not an earlier one", async () => {
    ipc.getMessages.mockResolvedValue([msg("1", "user", "earlier"), msg("2", "assistant", "ok")]);
    ipc.sendMessage.mockRejectedValueOnce({ kind: "other", message: "db down", retryable: true });
    const { result } = renderHook(() => useChat("a"));
    await waitFor(() => expect(result.current.messages).toHaveLength(2));
    act(() => result.current.send("new text", [], "m1"));
    await waitFor(() => expect(result.current.status).toBe("failed"));
    expect(result.current.pendingUser).toBe("new text");
    act(() => result.current.retry());
    expect(ipc.sendMessage).toHaveBeenLastCalledWith("a", "new text", [], "m1", expect.any(Function));
  });

  it("a promise resolving with status failed and no terminal event shows the error", async () => {
    const { result } = renderHook(() => useChat("a"));
    act(() => result.current.send("hi", [], "m1"));
    act(() => emit({ type: "text_delta", text: "He" }));
    ipc.getMessages.mockResolvedValue([msg("1", "user", "hi")]);
    await act(async () => finish(failed(OFFLINE)));
    expect(result.current.status).toBe("failed");
    expect(result.current.error).toEqual(OFFLINE);
    expect(result.current.streamingText).toBe("");
    expect(result.current.unsent).toBeNull();
    expect(result.current.messages).toHaveLength(1);
  });

  it("a terminal event before the promise changes nothing until the promise settles", async () => {
    const { result } = renderHook(() => useChat("a"));
    await waitFor(() => expect(ipc.getMessages).toHaveBeenCalledTimes(1));
    act(() => result.current.send("hi", [], "m1"));
    act(() => emit({ type: "text_delta", text: "Hel" }));
    await act(async () => emit({ type: "failed", error: OFFLINE }));
    await act(async () => emit({ type: "done" }));
    expect(result.current.status).toBe("streaming");
    expect(result.current.streamingText).toBe("Hel");
    expect(result.current.error).toBeNull();
    expect(ipc.getMessages).toHaveBeenCalledTimes(1);
    ipc.getMessages.mockResolvedValue([msg("1", "user", "hi"), msg("2", "assistant", "Hel", true)]);
    await act(async () => finish(interrupted("network")));
    expect(result.current.status).toBe("interrupted");
    expect(result.current.messages[1]).toMatchObject({ text: "Hel", interrupted: true });
  });

  it("does not start a second send while the chat is streaming", () => {
    const { result } = renderHook(() => useChat("a"));
    act(() => result.current.send("one", [], "m1"));
    act(() => result.current.send("two", [], "m1"));
    expect(ipc.sendMessage).toHaveBeenCalledTimes(1);
  });

  it("stop calls the stop command for the chat", () => {
    const { result } = renderHook(() => useChat("a"));
    act(() => result.current.send("hi", [], "m1"));
    act(() => result.current.stop());
    expect(ipc.stop).toHaveBeenCalledWith("a");
  });

  it("events for a chat that is no longer selected do not change the visible thread", async () => {
    const { result, rerender } = renderHook(({ id }) => useChat(id), { initialProps: { id: "a" } });
    act(() => result.current.send("hi", [], "m1"));
    rerender({ id: "b" });
    await waitFor(() => expect(result.current.status).toBe("idle"));
    act(() => emit({ type: "text_delta", text: "secret" }));
    expect(result.current.streamingText).toBe("");
    expect(result.current.status).toBe("idle");
    rerender({ id: "a" });
    expect(result.current.streamingText).toBe("secret");
    expect(result.current.status).toBe("streaming");
  });

  it("regenerate answers the last message again with regenerate true, on the given or last model", async () => {
    ipc.getMessages.mockResolvedValue([msg("u", "user", "q"), msg("a", "assistant", "old")]);
    const { result } = renderHook(() => useChat("a"));
    await waitFor(() => expect(result.current.messages).toHaveLength(2));
    act(() => result.current.regenerate("m2"));
    expect(ipc.retryTurn).toHaveBeenCalledWith("a", "m2", expect.any(Function), true);
    expect(result.current.status).toBe("streaming");
    ipc.getMessages.mockResolvedValue([msg("u", "user", "q"), msg("a2", "assistant", "new")]);
    await act(async () => finish(DONE));
    await waitFor(() => expect(result.current.status).toBe("idle"));
    expect(result.current.messages[1].text).toBe("new");
    act(() => result.current.regenerate());
    expect(ipc.retryTurn).toHaveBeenLastCalledWith("a", "m2", expect.any(Function), true);
  });

  it("regenerate does nothing without a model, and not while a reply is running", async () => {
    const { result } = renderHook(() => useChat("a"));
    act(() => result.current.regenerate());
    expect(ipc.retryTurn).not.toHaveBeenCalled();
    act(() => void result.current.send("hi", [], "m1"));
    act(() => result.current.regenerate("m2"));
    expect(ipc.retryTurn).not.toHaveBeenCalled();
  });
});
