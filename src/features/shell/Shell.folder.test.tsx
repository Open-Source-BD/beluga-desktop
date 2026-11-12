import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { model } from "../models/fixtures";
import Shell from "./Shell";

const ipc = vi.hoisted(() => ({
  listModels: vi.fn(),
  listChats: vi.fn(),
  getMessages: vi.fn(),
  createChat: vi.fn(),
  sendMessage: vi.fn(),
  keyStatus: vi.fn(),
  monthSpend: vi.fn(),
  chatUsage: vi.fn(),
  listPacks: vi.fn(),
  chatState: vi.fn(),
  chatFullness: vi.fn(),
  chooseChatFolder: vi.fn(),
  approvedFolders: vi.fn(),
  setChatFolder: vi.fn(),
  folderFiles: vi.fn(),
  attachFolderFile: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);

const CHAT = { id: "c1", title: "Login page", model_id: "test/model", updated_at: 1 };
const state = (folder: string | null) => ({ active_skills: [], folder, compacted: false });
const box = () => screen.findByLabelText("Message");

beforeEach(() => {
  vi.resetAllMocks();
  localStorage.clear();
  localStorage.setItem("lastModel", "test/model");
  ipc.listChats.mockResolvedValue([]);
  ipc.listModels.mockResolvedValue([model({ id: "test/model", name: "Paid One" })]);
  ipc.getMessages.mockResolvedValue([]);
  ipc.keyStatus.mockResolvedValue({ is_free_tier: false, free_daily: null, usage_usd: 0, limit_remaining_usd: 5 });
  ipc.monthSpend.mockResolvedValue(0);
  ipc.chatUsage.mockResolvedValue({ requests: 0, cost_usd: 0 });
  ipc.createChat.mockResolvedValue(CHAT);
  ipc.listPacks.mockResolvedValue([]);
  ipc.chatState.mockResolvedValue(state(null));
  ipc.chatFullness.mockResolvedValue({ percent: 0, context_length: 0 });
  ipc.approvedFolders.mockResolvedValue([]);
  ipc.sendMessage.mockReturnValue(new Promise(() => {}));
  ipc.folderFiles.mockResolvedValue({ entries: [{ path: "a.ts", is_dir: false, bytes: 3 }], total: 1, cut: false });
});

describe("Shell folder", () => {
  it("a new chat inherits lastFolder when it is the most recent approved folder", async () => {
    localStorage.setItem("lastFolder", "/work/app");
    ipc.approvedFolders.mockResolvedValue(["/work/app", "/other"]);
    ipc.setChatFolder.mockResolvedValue(undefined);
    ipc.chatState.mockResolvedValue(state("/work/app"));
    render(<Shell />);
    await userEvent.type(await box(), "hi{Enter}");
    await waitFor(() => expect(ipc.setChatFolder).toHaveBeenCalledWith("c1", "/work/app"));
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalled());
  });

  it("ignores a failed inheritance silently", async () => {
    localStorage.setItem("lastFolder", "/work/app");
    ipc.approvedFolders.mockResolvedValue(["/work/app"]);
    ipc.setChatFolder.mockRejectedValue({ kind: "x", message: "gone", retryable: false });
    render(<Shell />);
    await userEvent.type(await box(), "hi{Enter}");
    await waitFor(() => expect(ipc.sendMessage).toHaveBeenCalled());
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("the sidebar button creates the chat first, chooses a folder, remembers it and shows its name", async () => {
    ipc.chooseChatFolder.mockResolvedValue("/work/app");
    ipc.chatState.mockResolvedValueOnce(state(null)).mockResolvedValue(state("/work/app"));
    render(<Shell />);
    await userEvent.click(await screen.findByRole("button", { name: "Open a folder" }));
    await waitFor(() => expect(ipc.chooseChatFolder).toHaveBeenCalledWith("c1"));
    expect(ipc.createChat).toHaveBeenCalledTimes(1);
    expect(localStorage.getItem("lastFolder")).toBe("/work/app");
    expect(await screen.findByText("app")).toBeInTheDocument();
  });

  it("a cancelled dialog changes nothing", async () => {
    ipc.chooseChatFolder.mockResolvedValue(null);
    render(<Shell />);
    await userEvent.click(await screen.findByRole("button", { name: "Open a folder" }));
    await waitFor(() => expect(ipc.chooseChatFolder).toHaveBeenCalled());
    expect(localStorage.getItem("lastFolder")).toBeNull();
    expect(screen.getByRole("button", { name: "Open a folder" })).toBeInTheDocument();
  });

  it("Close folder clears the chat's folder", async () => {
    ipc.listChats.mockResolvedValue([CHAT]);
    ipc.chatState.mockResolvedValueOnce(state("/work/app")).mockResolvedValue(state(null));
    ipc.setChatFolder.mockResolvedValue(undefined);
    render(<Shell />);
    await userEvent.click(await screen.findByText("Login page"));
    await userEvent.click(await screen.findByRole("button", { name: "Close folder" }));
    expect(ipc.setChatFolder).toHaveBeenCalledWith("c1", null);
    expect(await screen.findByRole("button", { name: "Open a folder" })).toBeInTheDocument();
  });

  it("the no-folder row of the @ menu opens the dialog and then lists the folder's files", async () => {
    ipc.chooseChatFolder.mockResolvedValue("/work/app");
    ipc.chatState.mockResolvedValueOnce(state(null)).mockResolvedValue(state("/work/app"));
    render(<Shell />);
    await userEvent.type(await box(), "@");
    await userEvent.click(await screen.findByRole("option", { name: /Open a folder to mention its files/ }));
    await waitFor(() => expect(ipc.chooseChatFolder).toHaveBeenCalledWith("c1"));
    expect(await screen.findByText("a.ts")).toBeInTheDocument();
    expect((await box()) as HTMLTextAreaElement).toHaveValue("@");
  });
});
