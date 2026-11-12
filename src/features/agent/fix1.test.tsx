import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { StoredMessage, ToolStatus } from "../../lib/ipc";
import Thread from "../chat/Thread";
import type { TrailEntry } from "./trailEntries";

const row = (over: Partial<StoredMessage>): StoredMessage => ({
  id: "x", role: "assistant", text: "", attachments: [], interrupted: false, created_at: 1,
  kind: "message", seq: 1, tool_calls: [], tool_call_id: null, tool_meta: null, ...over,
});
const call = (id: string) => ({ id, name: "read_file", arguments: "{}" });
const tool = (rowId: string, id: string, summary: string, status: ToolStatus = "ok"): StoredMessage =>
  row({ id: rowId, role: "tool", kind: "tool", tool_call_id: id, tool_meta: { name: "read_file", status, summary, detail: null } });
const live = (over: Partial<TrailEntry>): TrailEntry => ({ callId: "call_0", name: "n", summary: "s", status: "running", ...over });
const props = {
  pendingUser: null, streamingText: "", status: "idle" as const, error: null,
  onRetry: vi.fn(), onSignInAgain: vi.fn(), onPickModel: vi.fn(),
};

describe("calls are matched by position", () => {
  const two = [
    row({ id: "u", role: "user", text: "q" }),
    row({ id: "a1", tool_calls: [call("call_0")] }),
    tool("t1", "call_0", "Read one"),
    row({ id: "a2", tool_calls: [call("call_0")] }),
  ];

  it("two replies that both use call_0: each shows its own line; the running one keeps its output", async () => {
    render(<Thread {...props} status="streaming" messages={two} live={[live({ summary: "Running two", output: "out2", liveId: 1 })]} />);
    expect(screen.getAllByText("Read one")).toHaveLength(1);
    await userEvent.click(screen.getByRole("button", { name: /Running two.*running/ }));
    expect(screen.getByText("out2")).toBeInTheDocument();
  });

  it("the second reply then shows its own result", () => {
    render(<Thread {...props} messages={[...two, tool("t2", "call_0", "Read two", "error")]} />);
    expect(screen.getByText("Read one")).toBeInTheDocument();
    expect(screen.getByText("Read two")).toBeInTheDocument();
    expect(screen.getByText("failed")).toBeInTheDocument();
  });

  it("a repeated id inside one reply pairs in order", () => {
    const msgs = [
      row({ id: "u", role: "user", text: "q" }),
      row({ id: "a", tool_calls: [call("c"), call("c")] }),
      tool("t1", "c", "First answer"),
      tool("t2", "c", "Second answer"),
    ];
    render(<Thread {...props} messages={msgs} />);
    const lines = screen.getAllByText(/answer/).map((e) => e.textContent);
    expect(lines).toEqual(["First answer", "Second answer"]);
  });

  it("a tool row whose id exists only in an older reply is an orphan and not shown", () => {
    const msgs = [...two, tool("t2", "call_0", "Read two"), row({ id: "a3", text: "done" }), tool("t3", "call_0", "Orphan late")];
    render(<Thread {...props} messages={msgs} />);
    expect(screen.queryByText("Orphan late")).toBeNull();
    expect(screen.getByText("Read two")).toBeInTheDocument();
  });

  it("only the last reply's unanswered call can be running; an earlier one is not run", () => {
    const msgs = [
      row({ id: "u", role: "user", text: "q" }),
      row({ id: "a1", tool_calls: [call("c1")] }),
      row({ id: "a2", tool_calls: [call("c2")] }),
    ];
    render(<Thread {...props} status="streaming" messages={msgs} />);
    expect(screen.getByText("not run")).toBeInTheDocument();
    expect(screen.getByText("Waiting to run")).toBeInTheDocument();
    expect(screen.queryByText("running")).toBeNull();
  });
});

describe("expansion and announcements", () => {
  it("a line stays open when its call moves from the live trail into the stored row", async () => {
    const u = row({ id: "u", role: "user", text: "q" });
    const l = live({ callId: "c1", summary: "Running x", output: "partial", liveId: 1 });
    const { rerender } = render(<Thread {...props} status="streaming" messages={[u]} live={[l]} />);
    await userEvent.click(screen.getByRole("button", { name: /Running x/ }));
    expect(screen.getByText("partial")).toBeInTheDocument();
    rerender(<Thread {...props} status="streaming" messages={[u, row({ id: "a", tool_calls: [call("c1")] })]} live={[l]} />);
    expect(screen.getByRole("button", { name: /Running x/ })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("partial")).toBeInTheDocument();
  });

  it("announces a finished call's summary once, and never a running one", () => {
    const u = row({ id: "u", role: "user", text: "q" });
    const running = live({ callId: "c1", summary: "Running y", liveId: 1, output: "chunk" });
    const { rerender } = render(<Thread {...props} status="streaming" messages={[u]} live={[running]} />);
    const region = screen.getByRole("status");
    expect(region).toHaveTextContent("");
    const done = { ...running, status: "error" as const, summary: "Ran y (exit 1)", output: undefined };
    rerender(<Thread {...props} status="streaming" messages={[u]} live={[done]} />);
    rerender(<Thread {...props} status="streaming" messages={[u]} live={[{ ...done }]} />);
    expect(region.textContent).toBe("Ran y (exit 1) failed");
  });
});
