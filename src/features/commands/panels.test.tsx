import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { commands } from "./registry";
import Help from "./Help";
import Usage from "./Usage";

describe("registry", () => {
  it("has the eight commands with their exact descriptions", () => {
    expect(commands.map((c) => [c.name, c.description])).toEqual([
      ["/new", "Start a new chat"],
      ["/model", "Switch the model for this chat"],
      ["/skills", "See and manage your skills"],
      ["/usage", "Show free requests left and what this chat has cost"],
      ["/compact", "Shorten this chat to save requests and cost"],
      ["/export", "Save this chat as a Markdown file"],
      ["/settings", "Open Settings"],
      ["/help", "List commands and keyboard shortcuts"],
    ]);
  });
});

describe("Help", () => {
  it("lists every command name and description and closes", async () => {
    const onClose = vi.fn();
    render(<Help onClose={onClose} />);
    for (const c of commands) {
      expect(screen.getByText(c.name)).toBeInTheDocument();
      expect(screen.getByText(c.description)).toBeInTheDocument();
    }
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("lists the keyboard shortcuts", () => {
    render(<Help onClose={vi.fn()} />);
    for (const key of ["Enter", "Shift+Enter", "Cmd/Ctrl+Enter", "Up arrow", "Esc", "/", "@", "Cmd/Ctrl+N"]) {
      expect(screen.getByText(key, { selector: "kbd" })).toBeInTheDocument();
    }
  });
});

describe("Usage", () => {
  it("shows free requests left, this chat and this month", () => {
    render(
      <Usage
        onClose={vi.fn()}
        freeLeft={38}
        freeLimit={50}
        chat={{ requests: 7, cost_usd: 0.5 }}
        monthSpend={1.234}
      />,
    );
    expect(screen.getByText("Free requests left today")).toBeInTheDocument();
    expect(screen.getByText("38 of 50")).toBeInTheDocument();
    expect(screen.getByText("This chat")).toBeInTheDocument();
    expect(screen.getByText("7 requests, $0.50")).toBeInTheDocument();
    expect(screen.getByText("Spent this month")).toBeInTheDocument();
    expect(screen.getByText("$1.23")).toBeInTheDocument();
  });

  it("copes with no allowance and no chat", () => {
    render(<Usage onClose={vi.fn()} freeLeft={null} freeLimit={null} chat={null} monthSpend={0} />);
    expect(screen.getByText("unknown")).toBeInTheDocument();
    expect(screen.getByText("No chat open")).toBeInTheDocument();
  });
});
