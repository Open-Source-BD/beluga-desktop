import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import Composer from "./Composer";

describe("Composer", () => {
  it("Enter sends and clears; Shift+Enter inserts a newline", async () => {
    const onSend = vi.fn();
    render(<Composer onSend={onSend} />);
    const box = screen.getByLabelText("Message");
    await userEvent.type(box, "line one{Shift>}{Enter}{/Shift}line two");
    expect(onSend).not.toHaveBeenCalled();
    expect(box).toHaveValue("line one\nline two");
    await userEvent.type(box, "{Enter}");
    expect(onSend).toHaveBeenCalledWith("line one\nline two");
    expect(box).toHaveValue("");
  });

  it("does not send empty or whitespace-only text", async () => {
    const onSend = vi.fn();
    render(<Composer onSend={onSend} />);
    const box = screen.getByLabelText("Message");
    await userEvent.type(box, "{Enter}");
    await userEvent.type(box, "   {Enter}");
    expect(onSend).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  });

  it("cannot send while streaming, and the control becomes Stop", async () => {
    const onSend = vi.fn();
    const onStop = vi.fn();
    render(<Composer onSend={onSend} onStop={onStop} streaming />);
    await userEvent.type(screen.getByLabelText("Message"), "more{Enter}");
    expect(onSend).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(onStop).toHaveBeenCalledTimes(1);
  });

  it("is disabled with a hint when no model is chosen", () => {
    render(<Composer onSend={vi.fn()} noModel />);
    expect(screen.getByLabelText("Message")).toBeDisabled();
    expect(screen.getByPlaceholderText("Pick a model to start.")).toBeInTheDocument();
  });

  it("uses the right placeholder and renders the model slot", () => {
    const { rerender } = render(<Composer onSend={vi.fn()} slots={{ model: <i>slot</i> }} />);
    expect(
      screen.getByPlaceholderText("Describe a feature, paste an error, or ask a question"),
    ).toBeInTheDocument();
    expect(screen.getByText("slot")).toBeInTheDocument();
    rerender(<Composer onSend={vi.fn()} hasMessages />);
    expect(screen.getByPlaceholderText("Reply")).toBeInTheDocument();
  });
});
