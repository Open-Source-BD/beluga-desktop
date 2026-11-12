import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { StoredMessage } from "../../lib/ipc";
import "./chat.css";
import Thread from "./Thread";

const base = {
  messages: [] as StoredMessage[],
  pendingUser: null,
  streamingText: "",
  status: "idle" as const,
  error: null,
  onRetry: vi.fn(),
  onSignInAgain: vi.fn(),
  onPickModel: vi.fn(),
};
const asst = (text: string, interrupted = false): StoredMessage => ({
  id: text, role: "assistant", text, attachments: [], interrupted, created_at: 1, kind: "message", seq: 1, tool_calls: [], tool_call_id: null, tool_meta: null,
});

describe("Thread scrolling", () => {
  let top = 0;
  const restore: Array<() => void> = [];
  const stub = (name: string, descriptor: PropertyDescriptor) => {
    Object.defineProperty(HTMLElement.prototype, name, { configurable: true, ...descriptor });
    restore.push(() => delete (HTMLElement.prototype as unknown as Record<string, unknown>)[name]);
  };
  const setup = () => {
    top = 0;
    stub("scrollHeight", { get: () => 1000 });
    stub("clientHeight", { get: () => 400 });
    stub("scrollTop", { get: () => top, set: (v: number) => { top = v; } });
  };
  afterEach(() => restore.splice(0).forEach((r) => r()));

  it("follows new text while at the bottom", () => {
    setup();
    const { container, rerender } = render(<Thread {...base} status="streaming" streamingText="a" />);
    const scroller = container.querySelector(".thread") as HTMLElement;
    top = 600;
    fireEvent.scroll(scroller);
    top = 0;
    rerender(<Thread {...base} status="streaming" streamingText="ab" />);
    expect(top).toBe(1000);
  });

  it("stops following once the user scrolls up", () => {
    setup();
    const { container, rerender } = render(<Thread {...base} status="streaming" streamingText="a" />);
    const scroller = container.querySelector(".thread") as HTMLElement;
    top = 100;
    fireEvent.scroll(scroller);
    rerender(<Thread {...base} status="streaming" streamingText="ab" />);
    expect(top).toBe(100);
  });
});

describe("Thread content", () => {
  it("renders code blocks in JetBrains Mono", () => {
    render(<Thread {...base} messages={[asst("```js\nlet x = 1;\n```")]} />);
    const code = screen.getByText("let x = 1;");
    expect(code.closest("pre")).not.toBeNull();
    expect(getComputedStyle(code.closest("pre") as Element).fontFamily).toContain("JetBrains Mono");
  });

  it("never renders raw HTML from the model", () => {
    const { container } = render(
      <Thread {...base} messages={[asst('<img src=x onerror="alert(1)"><script>1</script>ok')]} />,
    );
    expect(container.querySelector("img, script")).toBeNull();
  });

  it("shows links as text with the address", () => {
    const { container } = render(
      <Thread {...base} messages={[asst("[docs](https://example.com/a)")]} />,
    );
    expect(container.querySelector("a")).toBeNull();
    expect(screen.getByText(/docs \(https:\/\/example.com\/a\)/)).toBeInTheDocument();
  });

  it("marks an interrupted reply with Retry", async () => {
    const onRetry = vi.fn();
    render(<Thread {...base} onRetry={onRetry} messages={[asst("Part", true)]} />);
    expect(screen.getByText("Reply interrupted.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalled();
  });

  it("marks a pending message that was not sent", () => {
    render(<Thread {...base} pendingUser="typed" notSent status="failed" />);
    expect(screen.getByText("typed")).toBeInTheDocument();
    expect(screen.getByText("Not sent.")).toBeInTheDocument();
  });

  it("keeps a polite live region for the streaming reply", () => {
    const { container } = render(<Thread {...base} status="streaming" streamingText="Hi" />);
    expect(container.querySelector('[aria-live="polite"]')).toHaveTextContent("Hi");
  });

  it("says Waiting for the model… in the live region until the first text", () => {
    const { container, rerender } = render(<Thread {...base} pendingUser="hi" status="streaming" />);
    const live = container.querySelector('[aria-live="polite"]');
    expect(live).toHaveTextContent("Waiting for the model…");
    rerender(<Thread {...base} pendingUser="hi" status="streaming" streamingText="He" />);
    expect(screen.queryByText("Waiting for the model…")).toBeNull();
    expect(live).toHaveTextContent("He");
  });

  it("shows no waiting line when nothing is streaming", () => {
    render(<Thread {...base} status="idle" />);
    expect(screen.queryByText("Waiting for the model…")).toBeNull();
  });

  it("shows the error message as an alert with its button", async () => {
    const onSignInAgain = vi.fn();
    render(
      <Thread
        {...base}
        status="failed"
        onSignInAgain={onSignInAgain}
        error={{ kind: "unauthorized", message: "Key rejected", retryable: false }}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("Key rejected");
    await userEvent.click(screen.getByRole("button", { name: "Sign in again" }));
    expect(onSignInAgain).toHaveBeenCalled();
  });
});

describe("Thread attachments", () => {
  it("lists the names of a stored message's attachments under its text", () => {
    const m: StoredMessage = {
      id: "u1", role: "user", text: "Look", interrupted: false, created_at: 1, kind: "message", seq: 1, tool_calls: [], tool_call_id: null, tool_meta: null,
      attachments: [
        { name: "notes.md", mime: "text/markdown", data_base64: "aGk=" },
        { name: "p.png", mime: "image/png", data_base64: "AAAA" },
      ],
    };
    render(<Thread {...base} messages={[m]} />);
    const list = screen.getByRole("list", { name: "Attachments" });
    expect(list).toHaveTextContent("notes.md");
    expect(list).toHaveTextContent("p.png");
    expect(screen.getByText("Look")).toBeInTheDocument();
    expect(screen.queryByRole("img")).toBeNull();
  });
});

describe("Thread notes", () => {
  it("shows a note as a quiet line, not a message bubble", () => {
    const note: StoredMessage = {
      id: "n1", role: "system", text: "Summarised above this line", attachments: [],
      interrupted: false, created_at: 1, kind: "note", seq: 2, tool_calls: [], tool_call_id: null, tool_meta: null,
    };
    const { container } = render(<Thread {...base} messages={[asst("hello"), note]} />);
    const line = screen.getByText("Summarised above this line");
    expect(line).toHaveClass("note-line");
    expect(line.closest(".me, .reply")).toBeNull();
    expect(container.querySelectorAll(".reply")).toHaveLength(2); // hello + the empty live region
  });
});

describe("Thread edit pencil", () => {
  const user = (id: string, text: string, kind: "message" | "note" = "message"): StoredMessage => ({
    id, role: "user", text, attachments: [], interrupted: false, created_at: 1, kind, seq: 1, tool_calls: [], tool_call_id: null, tool_meta: null,
  });
  const msgs = [user("u1", "first"), asst("a1"), user("u2", "second"), user("n1", "a note", "note")];

  it("is on every user message, not on replies or notes, and calls onEdit with the message", async () => {
    const onEdit = vi.fn();
    render(<Thread {...base} messages={msgs} onEdit={onEdit} />);
    const pencils = screen.getAllByRole("button", { name: "Edit message" });
    expect(pencils).toHaveLength(2);
    await userEvent.click(pencils[0]);
    expect(onEdit).toHaveBeenCalledWith(msgs[0]);
  });

  it("is hidden while a reply is running", () => {
    render(<Thread {...base} messages={msgs} onEdit={vi.fn()} status="streaming" />);
    expect(screen.queryByRole("button", { name: "Edit message" })).toBeNull();
  });
});

describe("Thread message actions", () => {
  const user = (id: string, text: string): StoredMessage => ({ ...asst(text), id, role: "user" });
  let writeText: ReturnType<typeof vi.fn>;
  beforeEach(() => {
    writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  });

  it("gives the latest reply Copy and Retry, and an older reply only Copy", () => {
    render(<Thread {...base} onRetryReply={vi.fn()} messages={[user("u1", "q"), asst("old"), user("u2", "q2"), asst("new")]} />);
    expect(screen.getAllByRole("button", { name: "Retry" })).toHaveLength(1);
    const reply = screen.getByText("new").closest(".reply") as HTMLElement;
    expect(within(reply).getByRole("button", { name: "Copy" })).toBeVisible();
    expect(within(reply).getByRole("button", { name: "Retry" })).toBeVisible();
    const older = screen.getByText("old").closest(".reply") as HTMLElement;
    expect(within(older).getByRole("button", { name: "Copy" })).not.toBeVisible();
  });

  it("offers no Retry on the latest reply while a reply is running", () => {
    render(<Thread {...base} onRetryReply={vi.fn()} status="streaming" messages={[user("u1", "q"), asst("new")]} />);
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("offers no actions on a note", () => {
    const note: StoredMessage = { ...asst("a note"), id: "n1", kind: "note", role: "system" };
    render(<Thread {...base} onRetryReply={vi.fn()} messages={[user("u1", "q"), asst("reply"), note]} />);
    const line = screen.getByText("a note");
    expect(line.querySelector("button")).toBeNull();
    expect(line.closest(".reply, .me")).toBeNull();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("the Retry menu calls the retry handlers", async () => {
    const onRetryReply = vi.fn();
    const onRetryWithModel = vi.fn();
    render(
      <Thread {...base} onRetryReply={onRetryReply} onRetryWithModel={onRetryWithModel} messages={[user("u1", "q"), asst("new")]} />,
    );
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await userEvent.click(screen.getByRole("menuitem", { name: "Retry" }));
    expect(onRetryReply).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await userEvent.click(screen.getByRole("menuitem", { name: "Retry with another model…" }));
    expect(onRetryWithModel).toHaveBeenCalledTimes(1);
  });

  it("user messages show Copy and Edit message", async () => {
    const onEdit = vi.fn();
    const m = user("u1", "my question");
    render(<Thread {...base} onEdit={onEdit} messages={[m, asst("a")]} />);
    const mine = screen.getByText("my question").closest(".me") as HTMLElement;
    await userEvent.click(within(mine).getByRole("button", { name: "Edit message" }));
    expect(onEdit).toHaveBeenCalledWith(m);
    await userEvent.click(within(mine).getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith("my question");
  });

  it("each message exposes its local date and time", () => {
    const when = new Date(1_700_000_000 * 1000).toLocaleString();
    render(<Thread {...base} messages={[{ ...user("u1", "q"), created_at: 1_700_000_000 }, { ...asst("a"), created_at: 1_700_000_000 }]} />);
    const mine = screen.getByText("q").closest(".me") as HTMLElement;
    const reply = screen.getByText("a").closest(".reply") as HTMLElement;
    for (const el of [mine, reply]) {
      expect(el).toHaveAttribute("title", when);
      expect(el).toHaveAccessibleDescription(when);
    }
  });
});

describe("Thread code blocks", () => {
  let writeText: ReturnType<typeof vi.fn>;
  beforeEach(() => {
    writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  });

  it("shows the language and Copy code copies exactly the code", async () => {
    render(<Thread {...base} messages={[asst("```js\nlet x = 1;\nlet y = 2;\n```")]} />);
    expect(screen.getByText("js")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Copy code" }));
    expect(writeText).toHaveBeenCalledWith("let x = 1;\nlet y = 2;");
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
  });

  it("labels a block with no language 'text'", () => {
    render(<Thread {...base} messages={[asst("```\nplain\n```")]} />);
    expect(screen.getByText("text")).toBeInTheDocument();
  });

  it("a clipboard failure reads Could not copy", async () => {
    writeText.mockRejectedValue(new Error("no"));
    render(<Thread {...base} messages={[asst("```js\nx\n```")]} />);
    await userEvent.click(screen.getByRole("button", { name: "Copy code" }));
    expect(await screen.findByRole("button", { name: "Could not copy" })).toBeInTheDocument();
  });

  it("inline code is not a code block", () => {
    render(<Thread {...base} messages={[asst("use `npm test` here")]} />);
    expect(screen.queryByRole("button", { name: "Copy code" })).toBeNull();
  });
});

describe("Thread jump to latest", () => {
  let top = 0;
  const restore: Array<() => void> = [];
  const stub = (name: string, descriptor: PropertyDescriptor) => {
    Object.defineProperty(HTMLElement.prototype, name, { configurable: true, ...descriptor });
    restore.push(() => delete (HTMLElement.prototype as unknown as Record<string, unknown>)[name]);
  };
  afterEach(() => restore.splice(0).forEach((r) => r()));

  it("appears only when not following, and scrolls to the end", async () => {
    top = 0;
    stub("scrollHeight", { get: () => 1000 });
    stub("clientHeight", { get: () => 400 });
    stub("scrollTop", { get: () => top, set: (v: number) => { top = v; } });
    const { container } = render(<Thread {...base} messages={[asst("a")]} />);
    const scroller = container.querySelector(".thread") as HTMLElement;
    expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
    top = 100;
    fireEvent.scroll(scroller);
    await userEvent.click(await screen.findByRole("button", { name: "Jump to latest" }));
    expect(top).toBe(1000);
    top = 600;
    fireEvent.scroll(scroller);
    expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
  });
});
