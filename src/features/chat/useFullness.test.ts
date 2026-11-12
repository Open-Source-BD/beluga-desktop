import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => ({ chatFullness: vi.fn() }));
vi.mock("../../lib/ipc", () => ipc);
import { useFullness } from "./useFullness";

type P = { chat: string | null; model: string | null; skills: string; turn: number; draft: number };
const base: P = { chat: "c1", model: "m", skills: "", turn: 0, draft: 0 };
const hook = (p: P) => renderHook((q: P) => useFullness(q.chat, q.model, q.skills, q.turn, q.draft), { initialProps: p });

beforeEach(() => {
  vi.resetAllMocks();
  ipc.chatFullness.mockResolvedValue({ percent: 30, context_length: 1000 });
});

describe("useFullness", () => {
  it("reports the percent, and nothing when the context length is unknown", async () => {
    const { result, rerender } = hook(base);
    await waitFor(() => expect(result.current).toBe(30));
    ipc.chatFullness.mockResolvedValue({ percent: 0, context_length: 0 });
    rerender({ ...base, turn: 1 });
    await waitFor(() => expect(result.current).toBeNull());
  });

  it("asks again after a turn, a model change and a skill change", async () => {
    const { rerender } = hook(base);
    await waitFor(() => expect(ipc.chatFullness).toHaveBeenCalledTimes(1));
    rerender({ ...base, turn: 1 });
    await waitFor(() => expect(ipc.chatFullness).toHaveBeenCalledTimes(2));
    rerender({ ...base, turn: 1, model: "other" });
    await waitFor(() => expect(ipc.chatFullness).toHaveBeenCalledTimes(3));
    expect(ipc.chatFullness).toHaveBeenLastCalledWith("c1", "other", 0);
    rerender({ ...base, turn: 1, model: "other", skills: "a:b" });
    await waitFor(() => expect(ipc.chatFullness).toHaveBeenCalledTimes(4));
  });

  it("waits 300 ms after typing and sends only the last draft length", async () => {
    vi.useFakeTimers();
    try {
      const { rerender } = hook(base);
      await act(async () => {});
      expect(ipc.chatFullness).toHaveBeenCalledTimes(1);
      rerender({ ...base, draft: 5 });
      rerender({ ...base, draft: 9 });
      await act(async () => { vi.advanceTimersByTime(299); });
      expect(ipc.chatFullness).toHaveBeenCalledTimes(1);
      await act(async () => { vi.advanceTimersByTime(1); });
      expect(ipc.chatFullness).toHaveBeenCalledTimes(2);
      expect(ipc.chatFullness).toHaveBeenLastCalledWith("c1", "m", 9);
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows nothing without a chat", () => {
    const { result } = hook({ ...base, chat: null });
    expect(result.current).toBeNull();
    expect(ipc.chatFullness).not.toHaveBeenCalled();
  });
});
