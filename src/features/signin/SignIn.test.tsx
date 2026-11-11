import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AuthFinished } from "../../lib/ipc";
import SignIn from "./SignIn";
import signinCss from "./signin.css?inline";
import tokensCss from "../../app/tokens.css?inline";

const ipc = vi.hoisted(() => ({
  finished: undefined as undefined | ((r: AuthFinished) => void),
  signInStart: vi.fn(),
  signInCodeUrl: vi.fn(),
  signInCancel: vi.fn(),
  signInWithCode: vi.fn(),
  signInWithKey: vi.fn(),
  resolveKeyStorage: vi.fn(),
  openAuthUrl: vi.fn(),
}));

vi.mock("../../lib/ipc", () => ({
  signInStart: ipc.signInStart,
  signInCodeUrl: ipc.signInCodeUrl,
  signInCancel: ipc.signInCancel,
  signInWithCode: ipc.signInWithCode,
  signInWithKey: ipc.signInWithKey,
  resolveKeyStorage: ipc.resolveKeyStorage,
  openAuthUrl: ipc.openAuthUrl,
  onAuthFinished: vi.fn(async (h: (r: AuthFinished) => void) => {
    ipc.finished = h;
    return () => {};
  }),
}));

const finish = (r: AuthFinished) => act(() => ipc.finished?.(r));

async function renderSignIn(onSignedIn = vi.fn()) {
  render(<SignIn onSignedIn={onSignedIn} />);
  await waitFor(() => expect(ipc.finished).toBeDefined());
  return onSignedIn;
}

beforeEach(() => {
  ipc.finished = undefined;
  for (const k of [
    "signInStart",
    "signInCodeUrl",
    "signInCancel",
    "signInWithCode",
    "signInWithKey",
    "resolveKeyStorage",
    "openAuthUrl",
  ] as const) {
    ipc[k].mockReset().mockResolvedValue(undefined);
  }
  ipc.signInCodeUrl.mockResolvedValue({ auth_url: "https://openrouter.ai/auth?x" });
  ipc.signInStart.mockResolvedValue({ auth_url: "https://openrouter.ai/auth?start" });
});

describe("SignIn", () => {
  it("shows both buttons and the exact headings", async () => {
    await renderSignIn();
    expect(
      screen.getByRole("heading", { name: "A senior developer's habits, on a free model." }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "Connect your OpenRouter account" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Sign in with OpenRouter" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Paste a key I already have" })).toBeInTheDocument();
    expect(screen.getByText(/Beluga has no subscription/)).toBeInTheDocument();
    expect(screen.getByText(/Free models allow 50 requests a day/)).toBeInTheDocument();
  });

  it("starts browser sign-in and shows the waiting state", async () => {
    const user = userEvent.setup();
    await renderSignIn();
    await user.click(screen.getByRole("button", { name: "Sign in with OpenRouter" }));
    expect(ipc.signInStart).toHaveBeenCalledTimes(1);
    expect(screen.getByText("Finish signing in in your browser.")).toBeInTheDocument();
  });

  it("Cancel calls signInCancel and returns to the buttons", async () => {
    const user = userEvent.setup();
    await renderSignIn();
    await user.click(screen.getByRole("button", { name: "Sign in with OpenRouter" }));
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(ipc.signInCancel).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Sign in with OpenRouter" })).toBeInTheDocument();
  });

  it("explains a declined sign-in and lets both buttons work again", async () => {
    const user = userEvent.setup();
    await renderSignIn();
    await user.click(screen.getByRole("button", { name: "Sign in with OpenRouter" }));
    await finish({ ok: false, reason: "denied" });
    expect(screen.getByRole("alert")).toHaveTextContent(
      "OpenRouter sign-in was declined. Try again, or paste a key.",
    );
    await user.click(screen.getByRole("button", { name: "Sign in with OpenRouter" }));
    expect(ipc.signInStart).toHaveBeenCalledTimes(2);
    await finish({ ok: false, reason: "denied" });
    await user.click(screen.getByRole("button", { name: "Paste a key I already have" }));
    expect(screen.getByLabelText("OpenRouter key")).toBeInTheDocument();
  });

  it("explains a timed-out sign-in", async () => {
    const user = userEvent.setup();
    await renderSignIn();
    await user.click(screen.getByRole("button", { name: "Sign in with OpenRouter" }));
    await finish({ ok: false, reason: "timed_out" });
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Sign-in took too long and was cancelled. Try again.",
    );
  });

  it("calls onSignedIn when the browser sign-in succeeds", async () => {
    const onSignedIn = await renderSignIn();
    await finish({ ok: true, reason: null });
    expect(onSignedIn).toHaveBeenCalledTimes(1);
  });

  it("pasting a key calls signInWithKey from a password field", async () => {
    const user = userEvent.setup();
    const onSignedIn = await renderSignIn();
    await user.click(screen.getByRole("button", { name: "Paste a key I already have" }));
    const field = screen.getByLabelText("OpenRouter key");
    expect(field).toHaveAttribute("type", "password");
    await user.type(field, "sk-or-abc");
    await user.click(screen.getByRole("button", { name: "Use this key" }));
    expect(ipc.signInWithKey).toHaveBeenCalledWith("sk-or-abc");
    await waitFor(() => expect(onSignedIn).toHaveBeenCalled());
  });

  it("shows the UiError message for a rejected key and keeps the field", async () => {
    const user = userEvent.setup();
    const onSignedIn = await renderSignIn();
    ipc.signInWithKey.mockRejectedValue({
      kind: "unauthorized",
      message: "OpenRouter does not accept that key.",
      retryable: false,
    });
    await user.click(screen.getByRole("button", { name: "Paste a key I already have" }));
    await user.type(screen.getByLabelText("OpenRouter key"), "bad");
    await user.click(screen.getByRole("button", { name: "Use this key" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "OpenRouter does not accept that key.",
    );
    expect(screen.getByLabelText("OpenRouter key")).toBeInTheDocument();
    expect(onSignedIn).not.toHaveBeenCalled();
  });

  it("clears the pasted key from the field after submit", async () => {
    const user = userEvent.setup();
    await renderSignIn();
    ipc.signInWithKey.mockRejectedValue({ kind: "network", message: "Offline.", retryable: true });
    await user.click(screen.getByRole("button", { name: "Paste a key I already have" }));
    await user.type(screen.getByLabelText("OpenRouter key"), "sk-or-secret");
    await user.click(screen.getByRole("button", { name: "Use this key" }));
    await screen.findByRole("alert");
    expect(screen.getByLabelText("OpenRouter key")).toHaveValue("");
  });

  it("swaps to a one-time code field via the code URL", async () => {
    const user = userEvent.setup();
    const onSignedIn = await renderSignIn();
    await user.click(screen.getByRole("button", { name: "Paste a key I already have" }));
    await user.click(screen.getByRole("button", { name: "Get a one-time code instead" }));
    expect(ipc.openAuthUrl).toHaveBeenCalledWith("https://openrouter.ai/auth?x");
    const field = await screen.findByLabelText("One-time code");
    expect(field).toHaveAttribute("type", "text");
    expect(screen.queryByLabelText("OpenRouter key")).not.toBeInTheDocument();
    await user.type(field, "code123");
    await user.click(screen.getByRole("button", { name: "Use this code" }));
    expect(ipc.signInWithCode).toHaveBeenCalledWith("code123");
    await waitFor(() => expect(onSignedIn).toHaveBeenCalled());
  });

  it("in code mode, Get a new code fetches and opens a fresh code URL", async () => {
    const user = userEvent.setup();
    await renderSignIn();
    await user.click(screen.getByRole("button", { name: "Paste a key I already have" }));
    await user.click(screen.getByRole("button", { name: "Get a one-time code instead" }));
    await screen.findByLabelText("One-time code");
    ipc.signInCodeUrl.mockResolvedValue({ auth_url: "https://openrouter.ai/auth?y" });
    await user.click(screen.getByRole("button", { name: "Get a new code" }));
    expect(ipc.signInCodeUrl).toHaveBeenCalledTimes(2);
    expect(ipc.openAuthUrl).toHaveBeenLastCalledWith("https://openrouter.ai/auth?y");
    expect(screen.getByLabelText("One-time code")).toBeInTheDocument();
  });

  it("in code mode, Paste a key instead returns to the key field", async () => {
    const user = userEvent.setup();
    await renderSignIn();
    ipc.signInWithCode.mockRejectedValue({ kind: "other", message: "Expired.", retryable: false });
    await user.click(screen.getByRole("button", { name: "Paste a key I already have" }));
    await user.click(screen.getByRole("button", { name: "Get a one-time code instead" }));
    await user.type(await screen.findByLabelText("One-time code"), "old");
    await user.click(screen.getByRole("button", { name: "Use this code" }));
    await screen.findByRole("alert");
    await user.click(screen.getByRole("button", { name: "Paste a key instead" }));
    const field = screen.getByLabelText("OpenRouter key");
    expect(field).toHaveAttribute("type", "password");
    expect(screen.queryByLabelText("One-time code")).not.toBeInTheDocument();
    await user.type(field, "sk-or-abc");
    await user.click(screen.getByRole("button", { name: "Use this key" }));
    expect(ipc.signInWithKey).toHaveBeenCalledWith("sk-or-abc");
  });

  it("while waiting, Open the sign-in page again opens the stored URL", async () => {
    const user = userEvent.setup();
    await renderSignIn();
    await user.click(screen.getByRole("button", { name: "Sign in with OpenRouter" }));
    const again = await screen.findByRole("button", { name: "Open the sign-in page again" });
    expect(ipc.openAuthUrl).not.toHaveBeenCalled();
    await user.click(again);
    expect(ipc.openAuthUrl).toHaveBeenCalledWith("https://openrouter.ai/auth?start");
    expect(screen.getByText("Finish signing in in your browser.")).toBeInTheDocument();
  });

  it("opens the keychain dialog on reason keychain_unavailable and keeps the key for the session", async () => {
    const user = userEvent.setup();
    const onSignedIn = await renderSignIn();
    await user.click(screen.getByRole("button", { name: "Sign in with OpenRouter" }));
    await finish({ ok: false, reason: "keychain_unavailable" });
    const dialog = screen.getByRole("dialog", { name: "This computer has no keychain service" });
    expect(dialog).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Keep until I quit" })).toHaveFocus();
    await user.click(screen.getByRole("button", { name: "Keep until I quit" }));
    expect(ipc.resolveKeyStorage).toHaveBeenCalledWith("session");
    await waitFor(() => expect(onSignedIn).toHaveBeenCalled());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("opens the keychain dialog when signInWithKey rejects with keychain_unavailable; Save in a file saves to file", async () => {
    const user = userEvent.setup();
    const onSignedIn = await renderSignIn();
    ipc.signInWithKey.mockRejectedValue({
      kind: "keychain_unavailable",
      message: "No keychain.",
      retryable: false,
    });
    await user.click(screen.getByRole("button", { name: "Paste a key I already have" }));
    await user.type(screen.getByLabelText("OpenRouter key"), "sk-or-abc");
    await user.click(screen.getByRole("button", { name: "Use this key" }));
    await screen.findByRole("dialog");
    await user.click(screen.getByRole("button", { name: "Save in a file" }));
    expect(ipc.resolveKeyStorage).toHaveBeenCalledWith("file");
    await waitFor(() => expect(onSignedIn).toHaveBeenCalled());
  });

  it("keeps focus inside the dialog and ignores Escape", async () => {
    const user = userEvent.setup();
    await renderSignIn();
    await finish({ ok: false, reason: "keychain_unavailable" });
    await user.tab();
    expect(screen.getByRole("button", { name: "Save in a file" })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole("button", { name: "Keep until I quit" })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("shows a notice passed in", () => {
    render(<SignIn notice="Sign in again." onSignedIn={() => {}} />);
    expect(screen.getByText("Sign in again.")).toBeInTheDocument();
  });

  it("keeps the brand panel navy in light and dark themes", () => {
    const rule = signinCss.match(/\.signin-pitch\s*\{([^}]*)\}/);
    expect(rule?.[1]).toMatch(/background:\s*var\(--pitch\)/);
    const tokens = tokensCss;
    expect(tokens).toMatch(/:root\s*\{[^}]*--pitch:\s*#0f2b3c/);
    expect(tokens).toMatch(/data-theme="dark"\][^}]*--pitch:\s*#13303f/);
    render(<SignIn onSignedIn={() => {}} />);
    for (const theme of ["light", "dark"]) {
      document.documentElement.setAttribute("data-theme", theme);
      expect(document.querySelector(".signin-pitch")).toBeInTheDocument();
    }
    document.documentElement.removeAttribute("data-theme");
  });
});
