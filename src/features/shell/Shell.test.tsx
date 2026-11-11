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
  keyStatus: vi.fn(),
  monthSpend: vi.fn(),
  chatUsage: vi.fn(),
  authStatus: vi.fn(),
  signOut: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);

const FREE = model({ id: "test/free", name: "Free One", is_free: true, prompt_usd_per_mtok: 0, completion_usd_per_mtok: 0, context_length: 5000 });
const PAID = model({ id: "test/model", name: "Paid One" });

const FREE_STATUS = (used: number, limit: number) => ({
  is_free_tier: true,
  free_daily: { used, limit, remaining: limit - used },
  usage_usd: 0,
  limit_remaining_usd: null,
});

let emit: (e: EngineEvent) => void;

beforeEach(() => {
  vi.resetAllMocks();
  localStorage.clear();
  ipc.listChats.mockResolvedValue([]);
  ipc.listModels.mockResolvedValue([FREE, PAID]);
  ipc.getMessages.mockResolvedValue([]);
  ipc.keyStatus.mockResolvedValue(FREE_STATUS(12, 50));
  ipc.monthSpend.mockResolvedValue(0);
  ipc.chatUsage.mockResolvedValue({ requests: 7, cost_usd: 0 });
  ipc.authStatus.mockResolvedValue({ signed_in: true, storage: "keychain" });
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

  it("shows the gauge for a free model and spend and balance for a paid one", async () => {
    render(<Shell />);
    expect(await screen.findByRole("group", { name: "38 of 50 free requests left" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Free One/ }));
    await userEvent.click(screen.getByRole("option", { name: /Paid One/ }));
    expect(screen.getByText("Balance: no limit set")).toBeInTheDocument();
  });

  it("shows the chat usage in the header", async () => {
    ipc.listChats.mockResolvedValue([{ id: "c9", title: "Old chat", model_id: "test/free", updated_at: 1 }]);
    render(<Shell />);
    await userEvent.click(await screen.findByText("Old chat"));
    expect(await screen.findByText("This chat: 7 requests, $0.00")).toBeInTheDocument();
  });

  it("at zero on a free model blocks the box and shows LowAllowance; picking a model unblocks", async () => {
    ipc.keyStatus.mockResolvedValue(FREE_STATUS(50, 50));
    render(<Shell />);
    expect(await screen.findByText(/You have used today's free requests/)).toBeInTheDocument();
    expect(screen.getByLabelText("Message")).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: /Paid One/, description: undefined }));
    expect(screen.queryByText(/You have used today's free requests/)).toBeNull();
    expect(screen.getByLabelText("Message")).toBeEnabled();
  });

  it("at zero with a paid model selected does not block", async () => {
    localStorage.setItem("lastModel", "test/model");
    ipc.keyStatus.mockResolvedValue(FREE_STATUS(50, 50));
    render(<Shell />);
    expect(await screen.findByText("Balance: no limit set")).toBeInTheDocument();
    expect(screen.getByLabelText("Message")).toBeEnabled();
    expect(screen.queryByText(/You have used today's free requests/)).toBeNull();
  });

  it("refreshes after a turn and on window focus", async () => {
    localStorage.setItem("lastModel", "test/free");
    ipc.createChat.mockResolvedValue({ id: "c1", title: "New chat", model_id: "test/free", updated_at: 1 });
    render(<Shell />);
    await screen.findByRole("group", { name: "38 of 50 free requests left" });
    ipc.keyStatus.mockResolvedValue(FREE_STATUS(13, 50));
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(await screen.findByRole("group", { name: "37 of 50 free requests left" })).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("Message"), "hi{Enter}");
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalled());
    ipc.keyStatus.mockResolvedValue(FREE_STATUS(14, 50));
    act(() => emit({ type: "done" }));
    expect(await screen.findByRole("group", { name: "36 of 50 free requests left" })).toBeInTheDocument();
  });

  it("keeps the last numbers and says Could not refresh. when keyStatus fails", async () => {
    render(<Shell />);
    await screen.findByRole("group", { name: "38 of 50 free requests left" });
    ipc.keyStatus.mockRejectedValue({ kind: "network", message: "x" });
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(await screen.findByText("Could not refresh.")).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "38 of 50 free requests left" })).toBeInTheDocument();
  });

  it("opens Settings in place of the conversation and goes back to chat", async () => {
    render(<Shell />);
    await screen.findByLabelText("Message");
    await userEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(await screen.findByRole("button", { name: "Back to chat" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Message")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Back to chat" }));
    expect(screen.getByLabelText("Message")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Back to chat" })).toBeNull();
  });

  it("returns to sign-in after a confirmed sign out", async () => {
    ipc.signOut.mockResolvedValue(undefined);
    const onSignInAgain = vi.fn();
    render(<Shell onSignInAgain={onSignInAgain} />);
    await userEvent.click(screen.getByRole("button", { name: "Settings" }));
    await userEvent.click(await screen.findByRole("button", { name: "Sign out" }));
    await userEvent.click(screen.getByRole("button", { name: "Sign out" }));
    await waitFor(() => expect(onSignInAgain).toHaveBeenCalled());
  });

  it("refuses an image for a model that cannot read images and sends nothing", async () => {
    localStorage.setItem("lastModel", "test/model");
    render(<Shell />);
    const input = (await screen.findByLabelText("Attach files")) as HTMLInputElement;
    await userEvent.upload(input, new File([new Uint8Array([1])], "p.png", { type: "image/png" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "This model cannot read images. Pick a model that can, or remove the image.",
    );
    expect(ipc.sendMessage).not.toHaveBeenCalled();
  });

  it("sends attachments through the chat", async () => {
    localStorage.setItem("lastModel", "test/model");
    ipc.createChat.mockResolvedValue({ id: "c1", title: "New chat", model_id: "test/model", updated_at: 1 });
    render(<Shell />);
    const input = (await screen.findByLabelText("Attach files")) as HTMLInputElement;
    await userEvent.upload(input, new File(["hi"], "n.txt", { type: "text/plain" }));
    await screen.findByText("n.txt");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() =>
      expect(ipc.sendMessage).toHaveBeenCalledWith(
        "c1",
        "",
        [{ name: "n.txt", mime: "text/plain", data_base64: "aGk=" }],
        "test/model",
        expect.any(Function),
      ),
    );
  });
});
