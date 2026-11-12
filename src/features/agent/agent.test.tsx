import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { StoredMessage, ToolStatus } from "../../lib/ipc";
import Thread from "../chat/Thread";
import PlanList from "./PlanList";
import Trail from "./Trail";
import TrailLine from "./TrailLine";
import type { TrailEntry } from "./trailEntries";

const entry = (over: Partial<TrailEntry> = {}): TrailEntry => ({
  callId: "c1",
  name: "read_file",
  summary: "Read src/app.ts",
  status: "ok",
  ...over,
});

const row = (over: Partial<StoredMessage>): StoredMessage => ({
  id: "x", role: "assistant", text: "", attachments: [], interrupted: false, created_at: 1,
  kind: "message", seq: 1, tool_calls: [], tool_call_id: null, tool_meta: null, ...over,
});
const call = (id: string) => ({ id, name: "read_file", arguments: "{}" });
const toolRow = (id: string, summary: string, status: ToolStatus = "ok", detail: string | null = null): StoredMessage =>
  row({
    id: `t-${id}`, role: "tool", kind: "tool", tool_call_id: id,
    tool_meta: { name: "read_file", status, summary, detail },
  });

const thread = {
  pendingUser: null,
  streamingText: "",
  status: "idle" as const,
  error: null,
  onRetry: vi.fn(),
  onSignInAgain: vi.fn(),
  onPickModel: vi.fn(),
};

describe("TrailLine", () => {
  it("is a button that opens its detail and closes it again", async () => {
    render(<TrailLine entry={entry({ detail: "line one\n  line two" })} />);
    const b = screen.getByRole("button", { name: "Read src/app.ts" });
    expect(b).toHaveAttribute("aria-expanded", "false");
    b.focus();
    await userEvent.keyboard("{Enter}");
    expect(b).toHaveAttribute("aria-expanded", "true");
    const pre = screen.getByText((_, el) => el?.tagName === "PRE" && el.textContent === "line one\n  line two");
    expect(pre).toBeInTheDocument();
    await userEvent.keyboard(" ");
    expect(b).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText(/line one/)).toBeNull();
  });

  it("a line with nothing to show is plain text, not a dead button", () => {
    render(<TrailLine entry={entry()} />);
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByText("Read src/app.ts")).toBeInTheDocument();
  });

  it.each([
    ["running", "running"],
    ["error", "failed"],
    ["declined", "declined"],
    ["not_run", "not run"],
  ] as const)("marks %s in words", (status, word) => {
    render(<TrailLine entry={entry({ status, detail: "d" })} />);
    expect(screen.getByRole("button", { name: new RegExp(`Read src/app.ts.*${word}`) })).toBeInTheDocument();
  });

  it("an ok line shows the summary alone", () => {
    render(<TrailLine entry={entry({ detail: "d" })} />);
    expect(screen.getByRole("button")).toHaveAccessibleName("Read src/app.ts");
  });

  it("shows text as text: markup in a summary or detail is not interpreted", async () => {
    render(<TrailLine entry={entry({ summary: "Ran `<b>x</b>`", detail: "<img src=x>" })} />);
    await userEvent.click(screen.getByRole("button"));
    expect(document.querySelector("b, img")).toBeNull();
    expect(screen.getByText("<img src=x>")).toBeInTheDocument();
  });

  it("a running line with output shows it when opened", async () => {
    render(<TrailLine entry={entry({ status: "running", summary: "Running `ls`", output: "a\nb" })} />);
    await userEvent.click(screen.getByRole("button"));
    expect(screen.getByText((_, el) => el?.tagName === "PRE" && el.textContent === "a\nb")).toBeInTheDocument();
  });
});

describe("Trail", () => {
  it("lists its entries", () => {
    render(<Trail entries={[entry(), entry({ callId: "c2", summary: "Wrote a.ts" })]} />);
    expect(within(screen.getByRole("list", { name: "Steps" })).getAllByRole("listitem")).toHaveLength(2);
  });
});

describe("PlanList", () => {
  it("says each item's state in words", () => {
    render(
      <PlanList
        items={[
          { text: "Read the code", state: "done" },
          { text: "Write the test", state: "in_progress" },
          { text: "Ship", state: "pending" },
        ]}
      />,
    );
    const items = screen.getAllByRole("listitem");
    expect(items[0]).toHaveTextContent("Read the code, done");
    expect(items[1]).toHaveTextContent("Write the test, in progress");
    expect(items[2]).toHaveTextContent("Ship, to do");
    expect(screen.getByRole("region", { name: "Task checklist" })).toBeInTheDocument();
  });
});

describe("Thread with tool rows", () => {
  const stored = [
    row({ id: "u", role: "user", text: "do it" }),
    row({ id: "a1", text: "", tool_calls: [call("c1"), call("c2")] }),
    toolRow("c1", "Read src/app.ts"),
    toolRow("c2", "Wrote src/b.ts", "error", "disk full"),
    row({ id: "a2", text: "All done." }),
  ];

  it("a stored task renders two trail lines and one reply, and no tool bubble", () => {
    render(<Thread {...thread} messages={stored} />);
    expect(screen.getByText("Read src/app.ts")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Wrote src\/b.ts.*failed/ })).toBeInTheDocument();
    expect(screen.getAllByText("All done.")).toHaveLength(1);
    expect(document.querySelectorAll(".reply:not(.live)")).toHaveLength(1);
  });

  it("an orphan tool row is not rendered", () => {
    render(<Thread {...thread} messages={[...stored, toolRow("zz", "Orphan line")]} />);
    expect(screen.queryByText("Orphan line")).toBeNull();
  });

  it("message actions attach to the last assistant row with text", () => {
    render(<Thread {...thread} messages={stored} onRetryReply={vi.fn()} />);
    const reply = screen.getByText("All done.").closest(".reply") as HTMLElement;
    expect(within(reply).getByRole("button", { name: "Copy" })).toBeInTheDocument();
    expect(within(reply).queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("Retry is not offered when a tool row follows the latest user message", () => {
    render(<Thread {...thread} messages={stored} onRetryReply={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("Retry stays for a chat without tool rows", () => {
    render(
      <Thread
        {...thread}
        messages={[row({ id: "u", role: "user", text: "q" }), row({ id: "a", text: "answer" })]}
        onRetryReply={vi.fn()}
      />,
    );
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("notes still render as quiet lines", () => {
    render(<Thread {...thread} messages={[...stored, row({ id: "n", kind: "note", role: "system", text: "Paused: too many requests." })]} />);
    expect(screen.getByText("Paused: too many requests.")).toHaveClass("note-line");
  });

  it("a call with no result yet shows the live line (with its output) once the row is stored", async () => {
    const msgs = [row({ id: "u", role: "user", text: "q" }), row({ id: "a1", tool_calls: [call("c9")] })];
    const live = [entry({ callId: "c9", status: "running", summary: "Running `npm test`", output: "ok 1" })];
    render(<Thread {...thread} status="streaming" messages={msgs} live={live} />);
    await userEvent.click(screen.getByRole("button", { name: /Running `npm test`.*running/ }));
    expect(screen.getByText("ok 1")).toBeInTheDocument();
    expect(screen.getAllByText(/npm test/)).toHaveLength(1);
  });

  it("a live line already in the stored rows is not shown twice", () => {
    render(<Thread {...thread} status="streaming" messages={stored} live={[entry({ callId: "c1", status: "running" })]} />);
    expect(screen.getAllByText("Read src/app.ts")).toHaveLength(1);
  });

  it("live: a started call shows a running line before anything is stored", () => {
    render(<Thread {...thread} status="streaming" messages={[]} live={[entry({ status: "running", summary: "Reading a.ts" })]} />);
    expect(screen.getByText("Reading a.ts")).toBeInTheDocument();
    expect(screen.getByText("running")).toBeInTheDocument();
  });

  it("shows how long a rate-limit wait is", () => {
    render(<Thread {...thread} status="streaming" messages={[]} waiting={12} />);
    expect(screen.getByText("Waiting 12 s for the model…")).toBeInTheDocument();
  });

  it("a paused chat shows no error and no interrupted notice", () => {
    render(<Thread {...thread} status="paused" messages={stored} />);
    expect(screen.queryByText("Reply interrupted.")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
