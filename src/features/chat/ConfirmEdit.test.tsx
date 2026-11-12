import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import ConfirmEdit from "./ConfirmEdit";

describe("ConfirmEdit", () => {
  it("states the count and offers the two choices; Cancel has focus and Esc cancels", async () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    render(<ConfirmEdit count={3} onConfirm={onConfirm} onCancel={onCancel} />);
    expect(screen.getByText("Editing this message removes the 3 messages after it.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
    await userEvent.keyboard("{Escape}");
    expect(onCancel).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "Edit and remove" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });
});

describe("ConfirmEdit in a project chat", () => {
  it("adds the sentence about changes that stay when tool rows follow the message", () => {
    render(<ConfirmEdit count={3} toolsAfter onConfirm={vi.fn()} onCancel={vi.fn()} />);
    expect(screen.getByText("Files and commands already changed on your computer stay as they are.")).toBeInTheDocument();
  });
  it("does not add it otherwise", () => {
    render(<ConfirmEdit count={3} onConfirm={vi.fn()} onCancel={vi.fn()} />);
    expect(screen.queryByText(/already changed on your computer/)).toBeNull();
  });
});
