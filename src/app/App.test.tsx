import { act, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";

const ipc = vi.hoisted(() => ({
  revoked: undefined as undefined | (() => void),
  authStatus: vi.fn(),
}));

vi.mock("../lib/ipc", () => ({
  authStatus: ipc.authStatus,
  onAuthRevoked: vi.fn(async (h: () => void) => {
    ipc.revoked = h;
    return () => {};
  }),
  onAuthFinished: vi.fn(async () => () => {}),
  signInStart: vi.fn(),
  signInCancel: vi.fn(),
  signInCodeUrl: vi.fn(),
  signInWithCode: vi.fn(),
  signInWithKey: vi.fn(),
  resolveKeyStorage: vi.fn(),
  openAuthUrl: vi.fn(),
  listModels: vi.fn(async () => []),
  listChats: vi.fn(async () => []),
  getMessages: vi.fn(async () => []),
  createChat: vi.fn(),
  renameChat: vi.fn(),
  deleteChat: vi.fn(),
  sendMessage: vi.fn(),
  stop: vi.fn(),
  keyStatus: vi.fn(async () => ({ is_free_tier: true, free_daily: null, usage_usd: 0, limit_remaining_usd: null })),
  monthSpend: vi.fn(async () => 0),
  chatUsage: vi.fn(async () => ({ requests: 0, cost_usd: 0 })),
}));

beforeEach(() => {
  ipc.revoked = undefined;
  ipc.authStatus.mockReset();
});

describe("App", () => {
  it("shows nothing until auth status resolves", () => {
    ipc.authStatus.mockReturnValue(new Promise(() => {}));
    const { container } = render(<App />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows the sign-in screen when signed out", async () => {
    ipc.authStatus.mockResolvedValue({ signed_in: false, storage: null });
    render(<App />);
    expect(
      await screen.findByRole("button", { name: "Sign in with OpenRouter" }),
    ).toBeInTheDocument();
  });

  it("shows the shell when signed in", async () => {
    ipc.authStatus.mockResolvedValue({ signed_in: true, storage: "keychain" });
    render(<App />);
    expect(await screen.findByTestId("shell")).toBeInTheDocument();
  });

  it("returns to sign-in with a notice after the key is revoked", async () => {
    ipc.authStatus.mockResolvedValue({ signed_in: true, storage: "keychain" });
    render(<App />);
    await screen.findByTestId("shell");
    await waitFor(() => expect(ipc.revoked).toBeDefined());
    act(() => ipc.revoked?.());
    expect(
      await screen.findByText(
        "OpenRouter no longer accepts this key. Sign in again to continue. Your chats are still here.",
      ),
    ).toBeInTheDocument();
  });
});
