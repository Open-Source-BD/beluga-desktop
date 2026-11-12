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

describe("EmptyChat for a project", () => {
  it("shows the project line with the app name, and the top-up sentence for a free-tier key", () => {
    render(<EmptyChat project freeTier><p>box</p></EmptyChat>);
    expect(screen.getByText("Beluga can read files in this folder. It asks before changing a file or running a command.")).toBeInTheDocument();
    expect(screen.getByText(/A one-time \$10 top-up on OpenRouter raises that to 1,000 a day\./)).toBeInTheDocument();
  });
  it("shows neither without a folder, and no top-up for a paid key", () => {
    const { rerender } = render(<EmptyChat><p>box</p></EmptyChat>);
    expect(screen.queryByText(/can read files in this folder/)).toBeNull();
    expect(screen.queryByText(/top-up/)).toBeNull();
    rerender(<EmptyChat project><p>box</p></EmptyChat>);
    expect(screen.getByText(/can read files in this folder/)).toBeInTheDocument();
    expect(screen.queryByText(/top-up/)).toBeNull();
  });
});
