import { act, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { AuthFinished } from "../../lib/ipc";
import SignIn from "./SignIn";

const ipc = vi.hoisted(() => ({ finished: undefined as undefined | ((r: AuthFinished) => void) }));

vi.mock("../../app/brand", () => ({ APP_NAME: "Narwhal" }));
vi.mock("../../lib/ipc", () => ({
  signInStart: vi.fn(),
  signInCodeUrl: vi.fn(),
  signInCancel: vi.fn(),
  signInWithCode: vi.fn(),
  signInWithKey: vi.fn(),
  resolveKeyStorage: vi.fn(),
  openAuthUrl: vi.fn(),
  onAuthFinished: vi.fn(async (h: (r: AuthFinished) => void) => {
    ipc.finished = h;
    return () => {};
  }),
}));

describe("app name in copy", () => {
  it("comes from APP_NAME in the form and the keychain dialog", async () => {
    const { container } = render(<SignIn onSignedIn={() => {}} />);
    await waitFor(() => expect(ipc.finished).toBeDefined());
    act(() => ipc.finished?.({ ok: false, reason: "keychain_unavailable" }));
    const body = screen.getByRole("dialog").querySelector("#keydlg-body");
    expect(body?.textContent).toMatch(/^Narwhal can keep your key/);
    expect(screen.getByText(/Narwhal has no subscription/)).toBeInTheDocument();
    expect(container.textContent).not.toContain("Beluga");
  });
});
