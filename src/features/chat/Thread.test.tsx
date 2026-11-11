import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
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
  id: text, role: "assistant", text, attachments: [], interrupted, created_at: 1,
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

  it("keeps a polite live region for the streaming reply", () => {
    const { container } = render(<Thread {...base} status="streaming" streamingText="Hi" />);
    expect(container.querySelector('[aria-live="polite"]')).toHaveTextContent("Hi");
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
