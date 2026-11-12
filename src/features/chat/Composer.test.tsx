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
    expect(onSend).toHaveBeenCalledWith("line one\nline two", []);
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

  it("stays enabled while streaming: Enter still sends, and the control is Stop", async () => {
    const onSend = vi.fn();
    const onStop = vi.fn();
    render(<Composer onSend={onSend} onStop={onStop} streaming />);
    await userEvent.type(screen.getByLabelText("Message"), "more{Enter}");
    expect(onSend).toHaveBeenCalledWith("more", []);
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(onStop).toHaveBeenCalledTimes(1);
  });

  it("Esc stops a running reply, but not when the menu took the key or nothing is running", async () => {
    const onStop = vi.fn();
    const { rerender } = render(<Composer onSend={vi.fn()} onStop={onStop} />);
    await userEvent.type(screen.getByLabelText("Message"), "{Escape}");
    expect(onStop).not.toHaveBeenCalled();
    rerender(<Composer onSend={vi.fn()} onStop={onStop} streaming />);
    await userEvent.type(screen.getByLabelText("Message"), "{Escape}");
    expect(onStop).toHaveBeenCalledTimes(1);
    rerender(<Composer onSend={vi.fn()} onStop={onStop} streaming onKeyIntercept={() => true} />);
    await userEvent.type(screen.getByLabelText("Message"), "{Escape}");
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

describe("Composer when the send is rejected", () => {
  it("keeps the text and the attachment chips", async () => {
    const onSend = vi.fn().mockRejectedValue({ kind: "other", message: "db down", retryable: true });
    render(<Composer onSend={onSend} />);
    const input = screen.getByLabelText("Attach files") as HTMLInputElement;
    await userEvent.upload(input, new File(["hi"], "a.txt", { type: "text/plain" }));
    await screen.findByText("a.txt");
    await userEvent.type(screen.getByLabelText("Message"), "draft{Enter}");
    expect(onSend).toHaveBeenCalledTimes(1);
    expect(screen.getByLabelText("Message")).toHaveValue("draft");
    expect(screen.getByText("a.txt")).toBeInTheDocument();
  });

  it("clears once an asynchronous send is accepted", async () => {
    const onSend = vi.fn().mockResolvedValue(undefined);
    render(<Composer onSend={onSend} />);
    await userEvent.type(screen.getByLabelText("Message"), "draft{Enter}");
    expect(screen.getByLabelText("Message")).toHaveValue("");
  });
});

describe("Composer attachments", () => {
  const BLIND = "This model cannot read images. Pick a model that can, or remove the image.";
  const input = () => screen.getByLabelText("Attach files") as HTMLInputElement;
  const text = (name: string, body = "hi") => new File([body], name, { type: "text/plain" });
  const png = (name = "p.png") => new File([new Uint8Array([1, 2, 3])], name, { type: "image/png" });

  it("shows chosen files as chips that can be removed", async () => {
    render(<Composer onSend={vi.fn()} />);
    await userEvent.upload(input(), [text("a.txt"), text("b.txt")]);
    expect(await screen.findByText("a.txt")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Remove a.txt" }));
    expect(screen.queryByText("a.txt")).toBeNull();
    expect(screen.getByText("b.txt")).toBeInTheDocument();
  });

  it("has a keyboard reachable Attach button that opens the file picker", async () => {
    render(<Composer onSend={vi.fn()} />);
    const click = vi.spyOn(input(), "click");
    await userEvent.tab();
    await userEvent.tab();
    expect(screen.getByRole("button", { name: "Attach" })).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    expect(click).toHaveBeenCalled();
  });

  it("sends attachments with empty text, then clears the chips", async () => {
    const onSend = vi.fn();
    render(<Composer onSend={onSend} />);
    await userEvent.upload(input(), text("a.txt"));
    await screen.findByText("a.txt");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(onSend).toHaveBeenCalledWith("", [
      { name: "a.txt", mime: "text/plain", data_base64: "aGk=" },
    ]);
    expect(screen.queryByText("a.txt")).toBeNull();
  });

  it("refuses a sixth attachment but keeps the other five and can still send", async () => {
    const onSend = vi.fn();
    render(<Composer onSend={onSend} />);
    const six = ["1", "2", "3", "4", "5", "6"].map((n) => text(`f${n}.txt`));
    await userEvent.upload(input(), six);
    expect(await screen.findByRole("alert")).toHaveTextContent("You can attach up to 5 files.");
    expect(screen.getByText("f5.txt")).toBeInTheDocument();
    expect(screen.queryByText("f6.txt")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(onSend).toHaveBeenCalledTimes(1);
    expect(onSend.mock.calls[0][1]).toHaveLength(5);
  });

  it("shows oversize and unsupported refusals beside the chips", async () => {
    render(<Composer onSend={vi.fn()} />);
    await userEvent.upload(input(), [
      text("big.txt", "a".repeat(300_000)),
      new File(["PK"], "a.zip", { type: "application/zip" }),
      text("ok.txt"),
    ]);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("big.txt is too large to attach.");
    expect(alert).toHaveTextContent("a.zip is not a text file or image.");
    expect(screen.getByText("ok.txt")).toBeInTheDocument();
  });

  it("refuses an image at attach time when the model cannot read images", async () => {
    render(<Composer onSend={vi.fn()} acceptsImages={false} />);
    await userEvent.upload(input(), png());
    expect(await screen.findByRole("alert")).toHaveTextContent(BLIND);
    expect(screen.queryByText("p.png")).toBeNull();
  });

  it("disables send after switching to a model that cannot read images, until the image goes", async () => {
    const onSend = vi.fn();
    const { rerender } = render(<Composer onSend={onSend} acceptsImages />);
    await userEvent.upload(input(), png());
    await screen.findByText("p.png");
    await userEvent.type(screen.getByLabelText("Message"), "see");
    rerender(<Composer onSend={onSend} acceptsImages={false} />);
    expect(screen.getByRole("alert")).toHaveTextContent(BLIND);
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Remove p.png" }));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("button", { name: "Send" })).toBeEnabled();
    rerender(<Composer onSend={onSend} acceptsImages />);
  });
});
