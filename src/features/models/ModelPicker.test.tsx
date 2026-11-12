import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import ModelPicker from "./ModelPicker";
import { model } from "./fixtures";

const models = [
  model({ id: "x/alpha", name: "Alpha Paid" }),
  model({ id: "x/gemma", name: "Gemma Free", is_free: true, prompt_usd_per_mtok: 0, completion_usd_per_mtok: 0 }),
  model({ id: "y/beta", name: "Beta Paid", prompt_usd_per_mtok: 0.006, completion_usd_per_mtok: 2 }),
];

function Harness(props: { value?: string | null; loading?: boolean; error?: boolean; onRetry?: () => void; onChange?: (id: string) => void; startOpen?: boolean }) {
  const [open, setOpen] = useState(props.startOpen ?? false);
  return (
    <ModelPicker
      models={props.loading ? null : props.error ? [] : models}
      loadFailed={props.error ?? false}
      value={props.value ?? null}
      onChange={props.onChange ?? (() => {})}
      onRetry={props.onRetry ?? (() => {})}
      open={open}
      onOpenChange={setOpen}
    />
  );
}

describe("ModelPicker", () => {
  it("shows a disabled Loading models chip while the list loads", () => {
    render(<Harness loading />);
    expect(screen.getByRole("button", { name: /Loading models…/ })).toBeDisabled();
  });

  it("chip shows the model name and Free in the sonar colour for a free model", () => {
    render(<Harness value="x/gemma" />);
    const chip = screen.getByRole("button", { name: /Gemma Free/ });
    const tag = within(chip).getByText("Free");
    expect(tag).toHaveClass("free");
  });

  it("chip for a paid model has no Free tag", () => {
    render(<Harness value="x/alpha" />);
    expect(within(screen.getByRole("button", { name: /Alpha Paid/ })).queryByText("Free")).toBeNull();
  });

  it("groups under Free and Paid with price copy", async () => {
    render(<Harness value="x/alpha" />);
    await userEvent.click(screen.getByRole("button", { name: /Alpha Paid/ }));
    const free = screen.getByRole("group", { name: "Free" });
    const paid = screen.getByRole("group", { name: "Paid" });
    expect(within(free).getByRole("option", { name: /Gemma Free/ })).toHaveTextContent("Free");
    expect(within(paid).getByRole("option", { name: /Alpha Paid/ })).toHaveTextContent(
      "$0.50 in, $1.50 out per million tokens",
    );
    expect(within(paid).getByRole("option", { name: /Beta Paid/ })).toHaveTextContent("$0.006 in, $2.00 out per million tokens");
    expect(screen.getByRole("option", { name: /Alpha Paid/ })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("option", { name: /Beta Paid/ })).toHaveAttribute("aria-selected", "false");
  });

  it("filters by name and id, and says when nothing matches", async () => {
    render(<Harness startOpen />);
    const search = screen.getByPlaceholderText("Search models");
    await userEvent.type(search, "beta");
    expect(screen.getAllByRole("option")).toHaveLength(1);
    await userEvent.clear(search);
    await userEvent.type(search, "x/");
    expect(screen.getAllByRole("option")).toHaveLength(2);
    await userEvent.clear(search);
    await userEvent.type(search, "zzz");
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(screen.getByText("No models match.")).toBeInTheDocument();
  });

  it("arrow keys move, Enter selects and closes", async () => {
    const onChange = vi.fn();
    render(<Harness startOpen onChange={onChange} />);
    // order is Free first (Gemma), then Paid (Alpha, Beta)
    await userEvent.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    expect(onChange).toHaveBeenCalledWith("y/beta");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("Escape closes and returns focus to the chip", async () => {
    render(<Harness value="x/alpha" />);
    const chip = screen.getByRole("button", { name: /Alpha Paid/ });
    await userEvent.click(chip);
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(chip).toHaveFocus();
  });

  it("clicking outside closes", async () => {
    render(
      <div>
        <button>outside</button>
        <Harness startOpen />
      </div>,
    );
    await userEvent.click(screen.getByText("outside"));
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("clicking a row selects it", async () => {
    const onChange = vi.fn();
    render(<Harness startOpen onChange={onChange} />);
    await userEvent.click(screen.getByRole("option", { name: /Alpha Paid/ }));
    expect(onChange).toHaveBeenCalledWith("x/alpha");
  });

  it("shows the failure and a Try again button when the list could not load", async () => {
    const onRetry = vi.fn();
    render(<Harness error startOpen onRetry={onRetry} />);
    expect(screen.getByText("Could not load models.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(onRetry).toHaveBeenCalled();
  });

  it("renders several hundred models quickly", async () => {
    const many = Array.from({ length: 600 }, (_, i) => model({ id: `m/${i}`, name: `Model ${i}`, is_free: i % 3 === 0 }));
    const t0 = performance.now();
    render(
      <ModelPicker models={many} loadFailed={false} value="m/1" onChange={() => {}} onRetry={() => {}} open onOpenChange={() => {}} />,
    );
    expect(screen.getAllByRole("option")).toHaveLength(600);
    expect(performance.now() - t0).toBeLessThan(2000);
  });
});

describe("ModelPicker in a project chat", () => {
  const withTools = [
    model({ id: "x/tools", name: "Tools Paid" }),
    model({ id: "x/plain", name: "Plain Paid", supports_tools: false }),
    model({ id: "x/ftools", name: "Tools Free", is_free: true }),
  ];
  const picker = (toolsOnly: boolean) => (
    <ModelPicker models={withTools} loadFailed={false} value="x/tools" onChange={() => {}} onRetry={() => {}} open onOpenChange={() => {}} toolsOnly={toolsOnly} />
  );

  it("lists only models that support tools", () => {
    render(picker(true));
    expect(screen.getByRole("option", { name: /Tools Paid/ })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Tools Free/ })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Plain Paid/ })).toBeNull();
  });

  it("lists every model in an ordinary chat", () => {
    render(picker(false));
    expect(screen.getByRole("option", { name: /Plain Paid/ })).toBeInTheDocument();
  });
});
