import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { StoredMessage } from "../../lib/ipc";
import "./chat.css";
import MessageActions from "./MessageActions";

const message = (role: "user" | "assistant"): StoredMessage => ({
  id: "m1", role, text: "**Source** text", attachments: [], interrupted: false, created_at: 1, kind: "message", seq: 1, tool_calls: [], tool_call_id: null, tool_meta: null,
});
const handlers = () => ({
  onRetry: vi.fn(),
  onRetryWithModel: vi.fn(),
  onEdit: vi.fn(),
});
let writeText: ReturnType<typeof vi.fn>;

beforeEach(() => {
  writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
});
afterEach(() => vi.useRealTimers());

describe("MessageActions", () => {
  it("Copy writes the Markdown source, reads Copied, then Copy again after two seconds", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    render(<MessageActions role="assistant" message={message("assistant")} isLatest {...handlers()} />);
    await userEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith("**Source** text");
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
    act(() => void vi.advanceTimersByTime(2000));
    expect(screen.getByRole("button", { name: "Copy" })).toBeInTheDocument();
  });

  it("a clipboard failure reads Could not copy", async () => {
    writeText.mockRejectedValue(new Error("denied"));
    render(<MessageActions role="assistant" message={message("assistant")} isLatest {...handlers()} />);
    await userEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(await screen.findByRole("button", { name: "Could not copy" })).toBeInTheDocument();
  });

  it("the latest reply is always visible; an older one only after hover or focus", async () => {
    const { rerender } = render(
      <MessageActions role="assistant" message={message("assistant")} isLatest {...handlers()} />,
    );
    expect(screen.getByRole("button", { name: "Copy" })).toBeVisible();
    rerender(<MessageActions role="assistant" message={message("assistant")} isLatest={false} {...handlers()} />);
    expect(screen.getByRole("button", { name: "Copy" })).not.toBeVisible();
    await userEvent.tab();
    expect(screen.getByRole("button", { name: "Copy" })).toHaveFocus();
    expect(screen.getByRole("button", { name: "Copy" })).toBeVisible();
  });

  it("Retry is offered only on the latest reply and not while a reply is running", () => {
    const { rerender } = render(
      <MessageActions role="assistant" message={message("assistant")} isLatest={false} {...handlers()} />,
    );
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
    rerender(<MessageActions role="assistant" message={message("assistant")} isLatest running {...handlers()} />);
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
    rerender(<MessageActions role="assistant" message={message("assistant")} isLatest {...handlers()} />);
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("the Retry menu shows its three lines; Retry and Retry with another model call their handlers", async () => {
    const h = handlers();
    render(<MessageActions role="assistant" message={message("assistant")} isLatest {...h} />);
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(screen.getByRole("menuitem", { name: "Retry" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Retry with another model…" })).toBeInTheDocument();
    expect(screen.getByText("Uses one request")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("menuitem", { name: "Retry" }));
    expect(h.onRetry).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("menu")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await userEvent.click(screen.getByRole("menuitem", { name: "Retry with another model…" }));
    expect(h.onRetryWithModel).toHaveBeenCalledTimes(1);
  });

  it("Esc closes the Retry menu", async () => {
    render(<MessageActions role="assistant" message={message("assistant")} isLatest {...handlers()} />);
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(screen.getByRole("button", { name: "Retry" })).toHaveFocus();
  });

  it("a user message offers Copy and Edit message, and no Retry", async () => {
    const h = handlers();
    render(<MessageActions role="user" message={message("user")} isLatest {...h} />);
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Edit message" }));
    expect(h.onEdit).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith("**Source** text");
  });

  it("no Edit message button without an edit handler", () => {
    render(<MessageActions role="user" message={message("user")} isLatest={false} />);
    expect(screen.queryByRole("button", { name: "Edit message" })).toBeNull();
  });
});
