import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { StoredMessage } from "../../lib/ipc";
import Thread from "../chat/Thread";
import type { TrailEntry } from "./trailEntries";

const row = (over: Partial<StoredMessage>): StoredMessage => ({
  id: "x", role: "assistant", text: "", attachments: [], interrupted: false, created_at: 1,
  kind: "message", seq: 1, tool_calls: [], tool_call_id: null, tool_meta: null, ...over,
});
const call = (id: string) => ({ id, name: "n", arguments: "{}" });
const live = (over: Partial<TrailEntry>): TrailEntry => ({ callId: "c1", name: "n", summary: "s", status: "running", ...over });
const props = {
  pendingUser: null, streamingText: "", status: "streaming" as const, error: null,
  onRetry: vi.fn(), onSignInAgain: vi.fn(), onPickModel: vi.fn(),
};

describe("calls that have not started", () => {
  it("only the call a live line says started is running; the others wait", () => {
    const msgs = [row({ id: "u", role: "user", text: "q" }), row({ id: "a", tool_calls: [call("c1"), call("c2"), call("c3")] })];
    render(<Thread {...props} messages={msgs} live={[live({ summary: "Running first", liveId: 1 })]} />);
    expect(screen.getAllByText("running")).toHaveLength(1);
    expect(screen.getAllByText("Waiting to run")).toHaveLength(2);
  });
});

describe("a user row in the middle of a run", () => {
  it("the declined call, its tool row and the new user row: the next reply's call_0 still shows with its output", () => {
    const run = { base: new Set<string>(), key: "r1" };
    const msgs = [
      row({ id: "u", role: "user", text: "q" }),
      row({ id: "a1", tool_calls: [call("call_0")] }),
      row({ id: "t1", role: "tool", kind: "tool", tool_call_id: "call_0", tool_meta: { name: "n", status: "declined", summary: "Declined: x", detail: null } }),
      row({ id: "u2", role: "user", text: "do y instead" }),
      row({ id: "a2", tool_calls: [call("call_0")] }),
    ];
    render(<Thread {...props} run={run} messages={msgs} live={[live({ callId: "call_0", summary: "Running two", output: "out", liveId: 2 })]} />);
    expect(screen.getAllByText("Declined: x")).toHaveLength(1);
    expect(screen.getByText("Running two")).toBeInTheDocument();
  });
});

describe("announcements", () => {
  const u = row({ id: "u", role: "user", text: "q" });
  const done = (liveId: number): TrailEntry => live({ callId: `c${liveId}`, status: "ok", summary: "Ran z", liveId });

  it("does not read settled lines of a chat that is switched back to", () => {
    const { rerender } = render(<Thread {...props} chatId="a" messages={[u]} live={[]} />);
    rerender(<Thread {...props} chatId="b" messages={[u]} live={[done(1)]} />);
    expect(screen.getByRole("status")).toHaveTextContent("");
    rerender(<Thread {...props} chatId="b" messages={[u]} live={[done(1), done(2)]} />);
    expect(screen.getByRole("status").textContent).toBe("Ran z");
  });

  it("two identical announcements in a row are two nodes", () => {
    const { rerender } = render(<Thread {...props} chatId="a" messages={[u]} live={[]} />);
    rerender(<Thread {...props} chatId="a" messages={[u]} live={[done(1)]} />);
    const first = screen.getByRole("status").firstElementChild;
    rerender(<Thread {...props} chatId="a" messages={[u]} live={[done(1), done(2)]} />);
    const second = screen.getByRole("status").firstElementChild;
    expect(second).not.toBe(first);
    expect(second).toHaveTextContent("Ran z");
  });
});
