import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ActiveSkill } from "../../lib/ipc";
import ActiveSkills from "./ActiveSkills";

const skill = (over: Partial<ActiveSkill> & { id: string }): ActiveSkill => ({
  name: over.id,
  too_long: false,
  acts_on_computer: false,
  unavailable: false,
  ...over,
});

describe("ActiveSkills", () => {
  it("renders nothing without skills", () => {
    const { container } = render(<ActiveSkills skills={[]} onRemove={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows a chip per skill and removes by id", async () => {
    const onRemove = vi.fn();
    render(
      <ActiveSkills
        skills={[skill({ id: "p:one", name: "one" }), skill({ id: "p:two", name: "two" })]}
        onRemove={onRemove}
      />,
    );
    expect(screen.getAllByRole("listitem")).toHaveLength(2);
    await userEvent.click(screen.getByRole("button", { name: "Remove two" }));
    expect(onRemove).toHaveBeenCalledWith("p:two");
  });

  it("flags a too-long skill and an unavailable one, both still removable", async () => {
    const onRemove = vi.fn();
    render(
      <ActiveSkills
        skills={[skill({ id: "a", too_long: true }), skill({ id: "b", unavailable: true })]}
        onRemove={onRemove}
      />,
    );
    expect(screen.getByText("too long for this model")).toBeInTheDocument();
    expect(screen.getByText("not available")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Remove a" }));
    await userEvent.click(screen.getByRole("button", { name: "Remove b" }));
    expect(onRemove).toHaveBeenCalledTimes(2);
  });
});
