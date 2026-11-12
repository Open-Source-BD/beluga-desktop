import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TaskState } from "../../lib/ipc";

const ipc = vi.hoisted(() => ({ taskState: vi.fn(), onBackgroundChanged: vi.fn() }));
vi.mock("../../lib/ipc", () => ipc);
import { useTask } from "./useTask";

const state = (text: string, over: Partial<TaskState> = {}): TaskState => ({
  plan: [{ text, state: "pending" }],
  edits_allowed: false,
  background: [],
  project: true,
  waiting: null,
  running: false,
  ...over,
});
let bg: (chatId: string) => void;
const deferred = <T,>() => {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
};

beforeEach(() => {
  vi.resetAllMocks();
  ipc.onBackgroundChanged.mockImplementation((h: (id: string) => void) => {
    bg = h;
    return Promise.resolve(() => {});
  });
});

describe("useTask", () => {
  it("loads the chat's task state", async () => {
    ipc.taskState.mockResolvedValue(state("A", { edits_allowed: true }));
    const { result } = renderHook(() => useTask("a", 0));
    await waitFor(() => expect(result.current.plan).toEqual([{ text: "A", state: "pending" }]));
    expect(result.current.editsAllowed).toBe(true);
    expect(result.current.project).toBe(true);
    expect(ipc.taskState).toHaveBeenCalledWith("a");
  });

  it("refreshes when the tick changes and on a background change of this chat only", async () => {
    ipc.taskState.mockResolvedValue(state("A"));
    const { rerender } = renderHook(({ tick }) => useTask("a", tick), { initialProps: { tick: 0 } });
    await waitFor(() => expect(ipc.taskState).toHaveBeenCalledTimes(1));
    rerender({ tick: 1 });
    await waitFor(() => expect(ipc.taskState).toHaveBeenCalledTimes(2));
    act(() => bg("other"));
    expect(ipc.taskState).toHaveBeenCalledTimes(2);
    act(() => bg("a"));
    await waitFor(() => expect(ipc.taskState).toHaveBeenCalledTimes(3));
  });

  it("never shows another chat's state after switching, even when its answer arrives late", async () => {
    const slow = deferred<TaskState>();
    ipc.taskState.mockImplementation((id: string) => (id === "a" ? slow.promise : Promise.resolve(state("B"))));
    const { result, rerender } = renderHook(({ id }) => useTask(id, 0), { initialProps: { id: "a" } });
    rerender({ id: "b" });
    await waitFor(() => expect(result.current.plan[0]?.text).toBe("B"));
    await act(async () => slow.resolve(state("A")));
    expect(result.current.plan[0]?.text).toBe("B");
  });

  it("shows nothing for a chat whose state has not arrived yet", async () => {
    const slow = deferred<TaskState>();
    ipc.taskState.mockImplementation((id: string) => (id === "a" ? Promise.resolve(state("A")) : slow.promise));
    const { result, rerender } = renderHook(({ id }) => useTask(id, 0), { initialProps: { id: "a" } });
    await waitFor(() => expect(result.current.plan).toHaveLength(1));
    rerender({ id: "b" });
    expect(result.current.plan).toEqual([]);
  });

  it("an older answer for the same chat does not overwrite a newer one", async () => {
    const first = deferred<TaskState>();
    ipc.taskState.mockReturnValueOnce(first.promise).mockResolvedValue(state("NEW"));
    const { result, rerender } = renderHook(({ tick }) => useTask("a", tick), { initialProps: { tick: 0 } });
    rerender({ tick: 1 });
    await waitFor(() => expect(result.current.plan[0]?.text).toBe("NEW"));
    await act(async () => first.resolve(state("OLD")));
    expect(result.current.plan[0]?.text).toBe("NEW");
  });

  it("keeps what it shows when a refresh fails", async () => {
    ipc.taskState.mockResolvedValueOnce(state("A")).mockRejectedValue(new Error("x"));
    const { result, rerender } = renderHook(({ tick }) => useTask("a", tick), { initialProps: { tick: 0 } });
    await waitFor(() => expect(result.current.plan).toHaveLength(1));
    rerender({ tick: 1 });
    await waitFor(() => expect(ipc.taskState).toHaveBeenCalledTimes(2));
    expect(result.current.plan[0]?.text).toBe("A");
  });
});
