import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EngineEvent } from "../../lib/ipc";
import Shell from "./Shell";

const ipc = vi.hoisted(() => ({
  listChats: vi.fn(),
  getMessages: vi.fn(),
  createChat: vi.fn(),
  renameChat: vi.fn(),
  deleteChat: vi.fn(),
  sendMessage: vi.fn(),
  stop: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);

let emit: (e: EngineEvent) => void;

beforeEach(() => {
  vi.resetAllMocks();
  localStorage.clear();
  ipc.listChats.mockResolvedValue([]);
  ipc.getMessages.mockResolvedValue([]);
  ipc.sendMessage.mockImplementation((_c, _t, _a, _m, onEvent) => {
    emit = onEvent;
    return new Promise(() => {});
  });
});

describe("Shell", () => {
  it("disables the message box with a hint when no model is chosen", async () => {
    render(<Shell />);
    expect(await screen.findByPlaceholderText("Pick a model to start.")).toBeDisabled();
  });

  it("creates the chat, selects it, then sends the first message", async () => {
    localStorage.setItem("lastModel", "test/model");
    ipc.createChat.mockResolvedValue({ id: "c1", title: "New chat", model_id: "test/model", updated_at: 1 });
    render(<Shell />);
    await userEvent.type(await screen.findByLabelText("Message"), "hello{Enter}");
    await waitFor(() =>
      expect(ipc.sendMessage).toHaveBeenCalledWith("c1", "hello", [], "test/model", expect.any(Function)),
    );
    expect(ipc.createChat).toHaveBeenCalledWith("test/model");
    expect(screen.queryByText("What are we building?")).toBeNull();
    expect(await screen.findByText("hello")).toBeInTheDocument();
    act(() => emit({ type: "text_delta", text: "Hi there" }));
    expect(screen.getByText("Hi there")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
  });

  it("shows the greeting for a new chat", async () => {
    render(<Shell />);
    expect(await screen.findByText("What are we building?")).toBeInTheDocument();
  });
});
