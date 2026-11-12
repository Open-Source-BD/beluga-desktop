import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => ({ taskState: vi.fn(), onBackgroundChanged: vi.fn() }));
vi.mock("../../lib/ipc", () => ipc);
import { useTask } from "./useTask";

beforeEach(() => {
  vi.resetAllMocks();
  ipc.onBackgroundChanged.mockResolvedValue(() => {});
});

it("a forgotten chat's answers are dropped and it is not asked again", async () => {
  let resolve!: (v: unknown) => void;
  ipc.taskState.mockReturnValue(new Promise((r) => { resolve = r; }));
  const { result, rerender } = renderHook(({ tick }) => useTask("a", tick), { initialProps: { tick: 0 } });
  act(() => result.current.forget("a"));
  await act(async () => resolve({ plan: [{ text: "x", state: "pending" }], edits_allowed: false, background: [], project: true, waiting: null }));
  expect(result.current.plan).toEqual([]);
  const calls = ipc.taskState.mock.calls.length;
  rerender({ tick: 1 });
  await waitFor(() => expect(ipc.taskState).toHaveBeenCalledTimes(calls));
});
