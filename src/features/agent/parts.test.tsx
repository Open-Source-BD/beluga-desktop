import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { BgInfo } from "../../lib/ipc";
import Background from "./Background";
import EditsChip from "./EditsChip";

const ipc = vi.hoisted(() => ({ backgroundOutput: vi.fn(), stopBackground: vi.fn() }));
vi.mock("../../lib/ipc", () => ipc);

const bg = (over: Partial<BgInfo>): BgInfo => ({ id: "b1", command: "npm run dev", running: true, exit_code: null, started_at: 1, ...over });
beforeEach(() => {
  vi.resetAllMocks();
  ipc.backgroundOutput.mockResolvedValue("ready on 3000");
  ipc.stopBackground.mockResolvedValue(undefined);
});

describe("Background", () => {
  it("lists running commands exactly, and nothing for finished ones or none", () => {
    const { rerender, container } = render(<Background chatId="c" items={[bg({}), bg({ id: "b2", command: "old  cmd", running: false, exit_code: 0 })]} onError={vi.fn()} />);
    expect(screen.getByText("Running:")).toBeInTheDocument();
    const cmd = screen.getByText("npm run dev");
    expect(cmd.style.whiteSpace).toBe("pre");
    expect(screen.queryByText("old  cmd")).toBeNull();
    rerender(<Background chatId="c" items={[bg({ running: false })]} onError={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("Stop calls stopBackground for that command", async () => {
    render(<Background chatId="c" items={[bg({}), bg({ id: "b2", command: "tail -f log" })]} onError={vi.fn()} />);
    await userEvent.click(screen.getAllByRole("button", { name: "Stop" })[1]);
    expect(ipc.stopBackground).toHaveBeenCalledWith("c", "b2");
  });

  it("a refused Stop is shown through onError", async () => {
    ipc.stopBackground.mockRejectedValue({ kind: "other", message: "Could not stop it.", retryable: false });
    const onError = vi.fn();
    render(<Background chatId="c" items={[bg({})]} onError={onError} />);
    await userEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(onError).toHaveBeenCalledWith("Could not stop it."));
  });

  it("opens the output as text in a pre, refreshes it once a second while open, and closes", async () => {
    render(<Background chatId="c" items={[bg({})]} onError={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", { name: "npm run dev" }));
    const out = await screen.findByText("ready on 3000");
    expect(out.tagName).toBe("PRE");
    expect(ipc.backgroundOutput).toHaveBeenCalledWith("c", "b1");
    ipc.backgroundOutput.mockResolvedValue("<b>more</b>");
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      await act(async () => { await vi.advanceTimersByTimeAsync(1100); });
    } finally {
      vi.useRealTimers();
    }
    expect(await screen.findByText("<b>more</b>")).toBeInTheDocument();
    expect(document.querySelector("b")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "npm run dev" }));
    expect(screen.queryByText("<b>more</b>")).toBeNull();
  });
});

describe("EditsChip", () => {
  it("is the chip 'Edits allowed' and clicking it asks to switch it off", async () => {
    const onOff = vi.fn();
    render(<EditsChip onSwitchOff={onOff} />);
    await userEvent.click(screen.getByRole("button", { name: "Edits allowed" }));
    expect(onOff).toHaveBeenCalledTimes(1);
  });
});
