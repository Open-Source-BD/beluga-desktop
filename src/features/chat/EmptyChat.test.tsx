import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import EmptyChat from "./EmptyChat";

describe("EmptyChat", () => {
  it("shows the greeting, sub-line and its children", () => {
    render(<EmptyChat><p>box</p></EmptyChat>);
    expect(screen.getByRole("heading", { name: "What are we building?" })).toBeInTheDocument();
    expect(
      screen.getByText("Ask a question, paste an error, or think a problem through."),
    ).toBeInTheDocument();
    expect(screen.getByText("box")).toBeInTheDocument();
  });
});

describe("EmptyChat starter actions", () => {
  const starters = [
    { label: "Plan a feature before writing it", hint: "asks you questions, then writes a step-by-step plan", onPick: vi.fn() },
    { label: "Find and fix a bug", hint: "reproduces it first, then fixes the cause", onPick: vi.fn() },
  ];

  it("shows each action's label and secondary line, and picking calls its handler", async () => {
    render(<EmptyChat starters={starters}><p>box</p></EmptyChat>);
    expect(screen.getByText("asks you questions, then writes a step-by-step plan")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Find and fix a bug/ }));
    expect(starters[1].onPick).toHaveBeenCalledTimes(1);
    expect(starters[0].onPick).not.toHaveBeenCalled();
  });

  it("shows nothing when there are no actions", () => {
    render(<EmptyChat starters={[]}><p>box</p></EmptyChat>);
    expect(screen.queryByRole("list", { name: "Ways to start" })).toBeNull();
  });
});
