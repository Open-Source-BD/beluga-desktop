import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import PopMenu from "./PopMenu";
import type { Row } from "./match";

const rows: Row[] = [
  { id: "new", name: "/new", description: "Start a new chat", source: "Beluga", group: "Commands" },
  { id: "model", name: "/model", description: "Switch the model", source: "Beluga", group: "Commands" },
  { id: "p:a", name: "/p:a", description: "A skill", source: "p", group: "Skills" },
];

function Harness({ onPick, onClose }: { onPick: (r: Row, mode: string) => void; onClose: () => void }) {
  const [active, setActive] = useState(0);
  return (
    <div>
      <button>outside</button>
      <div>
        <PopMenu
          rows={rows}
          groups={["Commands", "Skills"]}
          active={active}
          onActiveChange={setActive}
          onPick={onPick}
          onClose={onClose}
          footer={<span>footer text</span>}
        />
      </div>
    </div>
  );
}

const setup = () => {
  const onPick = vi.fn();
  const onClose = vi.fn();
  render(<Harness onPick={onPick} onClose={onClose} />);
  const list = screen.getByRole("listbox");
  list.focus();
  return { onPick, onClose, list };
};

describe("PopMenu", () => {
  it("renders grouped options with the first selected and a footer", () => {
    setup();
    const options = screen.getAllByRole("option");
    expect(options).toHaveLength(3);
    expect(options[0]).toHaveAttribute("aria-selected", "true");
    expect(options[1]).toHaveAttribute("aria-selected", "false");
    expect(screen.getByText("Commands")).toBeInTheDocument();
    expect(screen.getByText("Skills")).toBeInTheDocument();
    expect(screen.getByText("footer text")).toBeInTheDocument();
  });

  it("arrows move the selection and stop at the ends", async () => {
    setup();
    await userEvent.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}");
    expect(screen.getAllByRole("option")[2]).toHaveAttribute("aria-selected", "true");
    await userEvent.keyboard("{ArrowUp}");
    expect(screen.getAllByRole("option")[1]).toHaveAttribute("aria-selected", "true");
  });

  it("Enter picks to run; Tab picks to complete; Esc closes", async () => {
    const { onPick, onClose } = setup();
    await userEvent.keyboard("{ArrowDown}{Enter}");
    expect(onPick).toHaveBeenLastCalledWith(rows[1], "run");
    await userEvent.keyboard("{Tab}");
    expect(onPick).toHaveBeenLastCalledWith(rows[1], "complete");
    await userEvent.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("a click picks to run", async () => {
    const { onPick } = setup();
    await userEvent.click(screen.getByRole("option", { name: /\/p:a/ }));
    expect(onPick).toHaveBeenCalledWith(rows[2], "run");
  });

  it("closes on a press outside, not inside", async () => {
    const { onClose } = setup();
    await userEvent.click(screen.getByText("footer text"));
    expect(onClose).not.toHaveBeenCalled();
    await userEvent.click(screen.getByText("outside"));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
