import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import Queued from "./Queued";

describe("Queued", () => {
  it("shows only the first 80 characters", () => {
    render(<Queued text={"x".repeat(100)} onRemove={vi.fn()} />);
    expect(screen.getByText(`Queued: ${"x".repeat(80)}`)).toBeInTheDocument();
  });

  it("Remove queued message calls onRemove", async () => {
    const onRemove = vi.fn();
    render(<Queued text="hi" onRemove={onRemove} />);
    await userEvent.click(screen.getByRole("button", { name: "Remove queued message" }));
    expect(onRemove).toHaveBeenCalledTimes(1);
  });
});
