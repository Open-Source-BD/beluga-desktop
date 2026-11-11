import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EngineEvent } from "../../lib/ipc";
import { model } from "../models/fixtures";
import Shell from "./Shell";

const ipc = vi.hoisted(() => ({
  listModels: vi.fn(),
  listChats: vi.fn(),
  getMessages: vi.fn(),
  createChat: vi.fn(),
  renameChat: vi.fn(),
  deleteChat: vi.fn(),
  sendMessage: vi.fn(),
  stop: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);

const FREE = model({ id: "test/free", name: "Free One", is_free: true, prompt_usd_per_mtok: 0, completion_usd_per_mtok: 0, context_length: 5000 });
const PAID = model({ id: "test/model", name: "Paid One" });

let emit: (e: EngineEvent) => void;

beforeEach(() => {
  vi.resetAllMocks();
  localStorage.clear();
  ipc.listChats.mockResolvedValue([]);
  ipc.listModels.mockResolvedValue([FREE, PAID]);
  ipc.getMessages.mockResolvedValue([]);
  ipc.sendMessage.mockImplementation((_c, _t, _a, _m, onEvent) => {
    emit = onEvent;
    return new Promise(() => {});
  });
});

describe("Shell", () => {
  it("disables the message box with a hint when no model is chosen", async () => {
    ipc.listModels.mockResolvedValue([PAID]);
    render(<Shell />);
    expect(await screen.findByPlaceholderText("Pick a model to start.")).toBeDisabled();
    expect(await screen.findByRole("listbox")).toBeInTheDocument();
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

  it("reads Loading models… and keeps the box disabled until the list arrives", async () => {
    let resolve: (m: unknown) => void = () => {};
    ipc.listModels.mockReturnValue(new Promise((r) => (resolve = r)));
    render(<Shell />);
    expect(screen.getByRole("button", { name: /Loading models…/ })).toBeDisabled();
    expect(screen.getByLabelText("Message")).toBeDisabled();
    await act(async () => resolve([FREE]));
    expect(await screen.findByRole("button", { name: /Free One/ })).toBeInTheDocument();
    expect(screen.getByLabelText("Message")).toBeEnabled();
  });

  it("starts a new chat on the default model when lastModel is not listed", async () => {
    localStorage.setItem("lastModel", "gone/away");
    render(<Shell />);
    expect(await screen.findByRole("button", { name: /Free One/ })).toBeInTheDocument();
  });

  it("choosing a model updates the chip and remembers it", async () => {
    render(<Shell />);
    await userEvent.click(await screen.findByRole("button", { name: /Free One/ }));
    await userEvent.click(screen.getByRole("option", { name: /Paid One/ }));
    expect(screen.getByRole("button", { name: /Paid One/ })).toBeInTheDocument();
    expect(localStorage.getItem("lastModel")).toBe("test/model");
  });

  it("selecting a chat switches to that chat's model", async () => {
    ipc.listChats.mockResolvedValue([{ id: "c9", title: "Old chat", model_id: "test/model", updated_at: 1 }]);
    render(<Shell />);
    expect(await screen.findByRole("button", { name: /Free One/ })).toBeInTheDocument();
    await userEvent.click(await screen.findByText("Old chat"));
    expect(await screen.findByRole("button", { name: /Paid One/ })).toBeInTheDocument();
  });

  it("selecting a chat whose model is no longer listed falls back", async () => {
    ipc.listChats.mockResolvedValue([{ id: "c9", title: "Old chat", model_id: "gone/away", updated_at: 1 }]);
    render(<Shell />);
    await screen.findByRole("button", { name: /Free One/ });
    await userEvent.click(await screen.findByText("Old chat"));
    expect(screen.getByRole("button", { name: /Free One/ })).toBeInTheDocument();
  });

  it("shows Could not load models. with Try again that forces a reload", async () => {
    ipc.listModels.mockRejectedValueOnce({ kind: "network", message: "x" });
    render(<Shell />);
    expect(await screen.findByText("Could not load models.")).toBeInTheDocument();
    ipc.listModels.mockResolvedValue([FREE]);
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(ipc.listModels).toHaveBeenLastCalledWith(true);
    expect(await screen.findByRole("button", { name: /Free One/ })).toBeInTheDocument();
    expect(ipc.listModels).toHaveBeenNthCalledWith(1, false);
  });

  it("Pick another model on the start error opens the picker", async () => {
    ipc.createChat.mockRejectedValue({ kind: "model_unavailable", message: "That model is not available." });
    render(<Shell />);
    await userEvent.type(await screen.findByLabelText("Message"), "hi{Enter}");
    await userEvent.click(await screen.findByRole("button", { name: "Pick another model" }));
    expect(await screen.findByRole("listbox")).toBeInTheDocument();
  });
});
