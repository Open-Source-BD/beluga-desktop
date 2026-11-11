import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EngineEvent, StoredMessage } from "../../lib/ipc";
import { useChat } from "./useChat";

const ipc = vi.hoisted(() => ({
  getMessages: vi.fn(),
  sendMessage: vi.fn(),
  stop: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);

const msg = (id: string, role: "user" | "assistant", text: string, interrupted = false): StoredMessage => ({
  id, role, text, attachments: [], interrupted, created_at: 1,
});

let emit: (e: EngineEvent) => void;
let finish: () => void;

beforeEach(() => {
  vi.resetAllMocks();
  ipc.getMessages.mockResolvedValue([]);
  ipc.stop.mockResolvedValue(undefined);
  ipc.sendMessage.mockImplementation(
    (_c: string, _t: string, _a: unknown, _m: string, onEvent: (e: EngineEvent) => void) => {
      emit = onEvent;
      return new Promise<void>((resolve) => {
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
      finish();
    });
    expect(result.current.status).toBe("idle");
    expect(result.current.streamingText).toBe("");
    expect(result.current.messages.map((m) => m.text)).toEqual(["hi", "Hello"]);
  });

  it("failed sets the error and keeps the user's message", async () => {
    const { result } = renderHook(() => useChat("a"));
    act(() => result.current.send("hi", [], "m1"));
    ipc.getMessages.mockResolvedValue([msg("1", "user", "hi")]);
    const error = { kind: "network" as const, message: "Offline", retryable: true };
    await act(async () => emit({ type: "failed", error }));
    expect(result.current.status).toBe("failed");
    expect(result.current.error).toEqual(error);
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
    await act(async () => emit({ type: "interrupted", reason: "stopped" }));
    expect(result.current.status).toBe("interrupted");
    expect(result.current.messages[1]).toMatchObject({ text: "Hel", interrupted: true });
    act(() => result.current.retry());
    expect(ipc.sendMessage).toHaveBeenLastCalledWith("a", "hi", [], "m1", expect.any(Function));
    expect(result.current.status).toBe("streaming");
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
});
