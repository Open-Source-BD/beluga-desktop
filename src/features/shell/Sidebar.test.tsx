import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import Sidebar from "./Sidebar";

const chats = [
  { id: "1", title: "Old", model_id: "m", updated_at: 1 },
  { id: "3", title: "Newest", model_id: "m", updated_at: 3 },
  { id: "2", title: "Middle", model_id: "m", updated_at: 2 },
];
const props = () => ({
  chats,
  selectedId: null,
  onSelect: vi.fn(),
  onNew: vi.fn(),
  onRename: vi.fn(),
  onDelete: vi.fn(),
  onOpenSettings: vi.fn(),
});

describe("Sidebar", () => {
  it("lists chats newest first under Earlier", () => {
    render(<Sidebar {...props()} />);
    expect(screen.getByText("Earlier")).toBeInTheDocument();
    const items = within(screen.getByRole("list")).getAllByRole("listitem");
    expect(items.map((i) => within(i).getAllByRole("button")[0].textContent)).toEqual([
      "Newest",
      "Middle",
      "Old",
    ]);
  });

  it("selects a chat, starts a new one, opens settings", async () => {
    const p = props();
    render(<Sidebar {...p} />);
    await userEvent.click(screen.getByRole("button", { name: "Middle" }));
    expect(p.onSelect).toHaveBeenCalledWith("2");
    await userEvent.click(screen.getByRole("button", { name: "New chat" }));
    expect(p.onNew).toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(p.onOpenSettings).toHaveBeenCalled();
  });

  it("renames in place", async () => {
    const p = props();
    render(<Sidebar {...p} />);
    await userEvent.click(screen.getByRole("button", { name: "Rename Old" }));
    const input = screen.getByLabelText("Chat name");
    await userEvent.clear(input);
    await userEvent.type(input, "Renamed{Enter}");
    expect(p.onRename).toHaveBeenCalledWith("1", "Renamed");
    expect(screen.queryByLabelText("Chat name")).toBeNull();
  });

  it("asks before deleting, and Keep chat cancels", async () => {
    const p = props();
    render(<Sidebar {...p} />);
    await userEvent.click(screen.getByRole("button", { name: "Delete Old" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("Delete this chat? This cannot be undone.");
    await userEvent.click(within(dialog).getByRole("button", { name: "Keep chat" }));
    expect(p.onDelete).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Delete Old" }));
    await userEvent.click(screen.getByRole("button", { name: "Delete chat" }));
    expect(p.onDelete).toHaveBeenCalledWith("1");
  });

  it("renders the footer slot", () => {
    render(<Sidebar {...props()} footer={<span>gauge</span>} />);
    expect(screen.getByText("gauge")).toBeInTheDocument();
  });

  it("shows the folder name with Change and Close folder, or an Open a folder button", async () => {
    const p = { ...props(), folder: "\\\\?\\C:\\Users\\me\\app", onOpenFolder: vi.fn(), onCloseFolder: vi.fn() };
    const { rerender } = render(<Sidebar {...p} />);
    expect(screen.getByText("Folder")).toBeInTheDocument();
    expect(screen.getByText("app")).toHaveAttribute("title", "C:\\Users\\me\\app");
    await userEvent.click(screen.getByRole("button", { name: "Change" }));
    expect(p.onOpenFolder).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "Close folder" }));
    expect(p.onCloseFolder).toHaveBeenCalledTimes(1);
    rerender(<Sidebar {...p} folder={null} />);
    expect(screen.queryByRole("button", { name: "Close folder" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Open a folder" }));
    expect(p.onOpenFolder).toHaveBeenCalledTimes(2);
  });
});
