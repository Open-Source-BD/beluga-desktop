import { renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useChatWatch } from "./useChatWatch";

const ipc = vi.hoisted(() => ({ taskState: vi.fn() }));
vi.mock("../../lib/ipc", () => ipc);

beforeEach(() => vi.resetAllMocks());

describe("the startup sweep", () => {
  it("asks at most 5 chats at a time, newest first, and only the 50 most recent", async () => {
    const chats = Array.from({ length: 60 }, (_, i) => ({ id: `c${i}`, updated_at: i }));
    let now = 0;
    let peak = 0;
    const order: string[] = [];
    ipc.taskState.mockImplementation(async (id: string) => {
      order.push(id);
      now++;
      peak = Math.max(peak, now);
      await new Promise((r) => setTimeout(r, 1));
      now--;
      return { running: false, waiting: null };
    });
    renderHook(() => useChatWatch(chats));
    await waitFor(() => expect(order).toHaveLength(50));
    await new Promise((r) => setTimeout(r, 20));
    expect(order).toHaveLength(50);
    expect(peak).toBeLessThanOrEqual(5);
    expect(order[0]).toBe("c59");
    expect(order).not.toContain("c9");
    expect(order).toContain("c10");
  });
});
