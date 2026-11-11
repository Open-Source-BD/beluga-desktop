import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { KeyStatus } from "../../lib/ipc";
import { model } from "../models/fixtures";
import Gauge from "./Gauge";
import LowAllowance from "./LowAllowance";

function status(used: number, limit: number, over: Partial<KeyStatus> = {}): KeyStatus {
  return {
    is_free_tier: true,
    free_daily: { used, limit, remaining: limit - used },
    usage_usd: 0,
    limit_remaining_usd: null,
    ...over,
  };
}
const marks = (c: HTMLElement) => c.querySelectorAll(".ticks i");
const spent = (c: HTMLElement) => c.querySelectorAll(".ticks i.u");

describe("Gauge on a free model", () => {
  it("38 of 50: 12 spent marks, 38 remaining, text", () => {
    const { container } = render(<Gauge status={status(12, 50)} monthSpend={0} usingFreeModel />);
    expect(screen.getByRole("group").firstElementChild).toHaveTextContent("38 of 50 free requests left");
    expect(marks(container)).toHaveLength(50);
    expect(spent(container)).toHaveLength(12);
    expect(screen.getByText("Spent this month: $0.00")).toBeInTheDocument();
    expect(screen.getByText(/^Resets at \d{1,2}:\d{2}/)).toBeInTheDocument();
  });

  it("950 of 1000: 50 marks, 2 spent", () => {
    const { container } = render(<Gauge status={status(50, 1000)} monthSpend={0} usingFreeModel />);
    expect(marks(container)).toHaveLength(50);
    expect(spent(container)).toHaveLength(2);
  });

  it("a mark counts as spent only when all its requests are used", () => {
    const { container } = render(<Gauge status={status(39, 1000)} monthSpend={0} usingFreeModel />);
    expect(spent(container)).toHaveLength(1);
  });

  it("remaining 10 is amber, remaining 11 is not", () => {
    const { container, rerender } = render(<Gauge status={status(40, 50)} monthSpend={0} usingFreeModel />);
    expect(container.querySelector(".gauge")).toHaveClass("low");
    rerender(<Gauge status={status(39, 50)} monthSpend={0} usingFreeModel />);
    expect(container.querySelector(".gauge")).not.toHaveClass("low");
  });

  it("hides the marks from assistive technology and exposes one summary", () => {
    const { container } = render(<Gauge status={status(12, 50)} monthSpend={0} usingFreeModel />);
    expect(container.querySelector(".ticks")).toHaveAttribute("aria-hidden", "true");
    expect(screen.getByRole("group", { name: "38 of 50 free requests left" })).toBeInTheDocument();
  });

  it("free_daily null: unknown, no marks", () => {
    const { container } = render(<Gauge status={status(0, 0, { free_daily: null })} monthSpend={0} usingFreeModel />);
    expect(screen.getByText("Free requests left: unknown")).toBeInTheDocument();
    expect(marks(container)).toHaveLength(0);
  });

  it("shows Could not refresh. when the last refresh failed", () => {
    render(<Gauge status={status(12, 50)} monthSpend={0} usingFreeModel refreshFailed />);
    expect(screen.getByText("Could not refresh.")).toBeInTheDocument();
  });
});

describe("Gauge on a paid model", () => {
  it("shows spend and balance, no marks", () => {
    const { container } = render(
      <Gauge status={status(5, 50, { limit_remaining_usd: 4.5 })} monthSpend={1.234} usingFreeModel={false} />,
    );
    expect(marks(container)).toHaveLength(0);
    expect(screen.getByText("Spent this month: $1.23")).toBeInTheDocument();
    expect(screen.getByText("Balance: $4.50")).toBeInTheDocument();
  });
  it("says no limit set when there is no cap", () => {
    render(<Gauge status={status(5, 50)} monthSpend={0} usingFreeModel={false} />);
    expect(screen.getByText("Balance: no limit set")).toBeInTheDocument();
  });
});

describe("LowAllowance", () => {
  const list = [
    model({ id: "a", name: "Alpha", prompt_usd_per_mtok: 3, completion_usd_per_mtok: 3 }),
    model({ id: "b", name: "Beta", prompt_usd_per_mtok: 1, completion_usd_per_mtok: 1 }),
    model({ id: "c", name: "Gamma", prompt_usd_per_mtok: 2, completion_usd_per_mtok: 2 }),
    model({ id: "d", name: "Delta", prompt_usd_per_mtok: 4, completion_usd_per_mtok: 4 }),
    model({ id: "f", name: "Free", is_free: true, prompt_usd_per_mtok: 0, completion_usd_per_mtok: 0 }),
  ];
  it("shows the copy and exactly three cheapest paid models, cheapest first", async () => {
    const onPick = vi.fn();
    render(<LowAllowance models={list} onPick={onPick} />);
    expect(screen.getByText(/^You have used today's free requests\. They reset at \d{1,2}:\d{2}/)).toBeInTheDocument();
    const buttons = screen.getAllByRole("button");
    expect(buttons.map((b) => b.textContent)).toEqual([
      expect.stringContaining("Beta"),
      expect.stringContaining("Gamma"),
      expect.stringContaining("Alpha"),
    ]);
    expect(screen.getByText("A one-time $10 top-up on OpenRouter raises the free limit to 1,000 requests a day.")).toBeInTheDocument();
    await userEvent.click(buttons[0]);
    expect(onPick).toHaveBeenCalledWith("b");
  });
  it("with no paid tool models shows only the two sentences", () => {
    render(<LowAllowance models={[list[4]]} onPick={() => {}} />);
    expect(screen.queryAllByRole("button")).toHaveLength(0);
    expect(screen.getByText(/They reset at/)).toBeInTheDocument();
    expect(screen.getByText(/one-time \$10 top-up/)).toBeInTheDocument();
  });
});
