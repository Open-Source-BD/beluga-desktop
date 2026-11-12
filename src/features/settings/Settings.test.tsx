import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import Settings from "./Settings";

const ipc = vi.hoisted(() => ({
  authStatus: vi.fn(),
  signOut: vi.fn(),
  listPacks: vi.fn(),
  packLicenses: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);

const setAppearance = vi.hoisted(() => vi.fn());
const getAppearance = vi.hoisted(() => vi.fn());
vi.mock("../../app/appearance", () => ({ setAppearance, getAppearance }));

beforeEach(() => {
  vi.resetAllMocks();
  getAppearance.mockReturnValue("system");
  ipc.authStatus.mockResolvedValue({ signed_in: true, storage: "keychain" });
});

describe("Settings", () => {
  it("marks the current appearance and sets a new one", async () => {
    render(<Settings onBack={vi.fn()} onSignedOut={vi.fn()} />);
    expect(screen.getByRole("button", { name: "System" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Dark" })).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(screen.getByRole("button", { name: "Dark" }));
    expect(setAppearance).toHaveBeenCalledWith("dark");
    expect(screen.getByRole("button", { name: "Dark" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "System" })).toHaveAttribute("aria-pressed", "false");
  });

  it.each([
    ["keychain", "Key storage: This computer's keychain"],
    ["session", "Key storage: Until you quit"],
    ["file", "Key storage: A file on this computer"],
  ])("describes %s key storage", async (storage, line) => {
    ipc.authStatus.mockResolvedValue({ signed_in: true, storage });
    render(<Settings onBack={vi.fn()} onSignedOut={vi.fn()} />);
    expect(await screen.findByText(line)).toBeInTheDocument();
  });

  it("goes back to the chat", async () => {
    const onBack = vi.fn();
    render(<Settings onBack={onBack} onSignedOut={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", { name: "Back to chat" }));
    expect(onBack).toHaveBeenCalled();
  });

  it("asks first, then signs out and returns to sign-in", async () => {
    ipc.signOut.mockResolvedValue(undefined);
    const onSignedOut = vi.fn();
    render(<Settings onBack={vi.fn()} onSignedOut={onSignedOut} />);
    await userEvent.click(screen.getByRole("button", { name: "Sign out" }));
    expect(screen.getByText("Sign out? Your chats stay on this computer.")).toBeInTheDocument();
    expect(ipc.signOut).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Sign out" }));
    expect(ipc.signOut).toHaveBeenCalledTimes(1);
    await vi.waitFor(() => expect(onSignedOut).toHaveBeenCalled());
  });

  it("cancelling the confirmation does not sign out", async () => {
    render(<Settings onBack={vi.fn()} onSignedOut={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", { name: "Sign out" }));
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(ipc.signOut).not.toHaveBeenCalled();
    expect(screen.queryByText(/Your chats stay/)).toBeNull();
  });

  it("shows the message when sign out fails and does not return to sign-in", async () => {
    ipc.signOut.mockRejectedValue({ kind: "other", message: "Could not remove the key.", retryable: false });
    const onSignedOut = vi.fn();
    render(<Settings onBack={vi.fn()} onSignedOut={onSignedOut} />);
    await userEvent.click(screen.getByRole("button", { name: "Sign out" }));
    await userEvent.click(screen.getByRole("button", { name: "Sign out" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not remove the key.");
    expect(onSignedOut).not.toHaveBeenCalled();
  });

  it("opens the Open-source licenses view and comes back", async () => {
    ipc.listPacks.mockResolvedValue([{ id: "p", title: "p", license: "MIT", origin: "built_in", skills: [] }]);
    ipc.packLicenses.mockResolvedValue([{ name: "LICENSE", text: "MIT text" }]);
    render(<Settings onBack={vi.fn()} onSignedOut={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", { name: "Open-source licenses" }));
    expect(await screen.findByText("MIT text")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Back to Settings" }));
    expect(screen.getByRole("button", { name: "Open-source licenses" })).toBeInTheDocument();
  });

  it("has a 'Send with Cmd/Ctrl+Enter' switch stored in localStorage", async () => {
    localStorage.clear();
    const onSendKeyChange = vi.fn();
    render(<Settings onBack={vi.fn()} onSignedOut={vi.fn()} onSendKeyChange={onSendKeyChange} />);
    const sw = screen.getByRole("switch", { name: "Send with Cmd/Ctrl+Enter" });
    expect(sw).toHaveAttribute("aria-checked", "false");
    await userEvent.click(sw);
    expect(sw).toHaveAttribute("aria-checked", "true");
    expect(localStorage.getItem("sendWithModifier")).toBe("true");
    expect(onSendKeyChange).toHaveBeenCalledWith(true);
    await userEvent.click(sw);
    expect(localStorage.getItem("sendWithModifier")).toBe("false");
    expect(onSendKeyChange).toHaveBeenLastCalledWith(false);
  });

  it("starts the switch from the stored setting", () => {
    localStorage.setItem("sendWithModifier", "true");
    render(<Settings onBack={vi.fn()} onSignedOut={vi.fn()} />);
    expect(screen.getByRole("switch", { name: "Send with Cmd/Ctrl+Enter" })).toHaveAttribute("aria-checked", "true");
    localStorage.clear();
  });
});
