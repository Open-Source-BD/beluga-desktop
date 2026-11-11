import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
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
