import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import Fullness from "./Fullness";

describe("Fullness", () => {
  it("shows the percent", () => {
    render(<Fullness percent={42} />);
    expect(screen.getByText("Chat is 42% full")).toBeInTheDocument();
    expect(screen.queryByText("Use /compact to shorten it")).toBeNull();
  });

  it("69 is not amber and has no hint", () => {
    const { container } = render(<Fullness percent={69} />);
    expect(container.querySelector(".fullness")).not.toHaveClass("high");
    expect(screen.queryByText("Use /compact to shorten it")).toBeNull();
  });

  it("70 is amber with the /compact hint", () => {
    const { container } = render(<Fullness percent={70} />);
    expect(container.querySelector(".fullness")).toHaveClass("high");
    expect(screen.getByText("Use /compact to shorten it")).toBeInTheDocument();
  });
});
