import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import Licenses from "./Licenses";

const ipc = vi.hoisted(() => ({ listPacks: vi.fn(), packLicenses: vi.fn() }));
vi.mock("../../lib/ipc", () => ipc);

const pk = (id: string, license: string) => ({ id, title: id, license, origin: "built_in", skills: [] });

beforeEach(() => {
  vi.resetAllMocks();
  ipc.listPacks.mockResolvedValue([pk("superpowers", "MIT"), pk("anthropic", "Apache-2.0")]);
  ipc.packLicenses.mockImplementation(async (id: string) => [{ name: "LICENSE", text: `Text of ${id} license` }]);
});

describe("Licenses", () => {
  it("lists every pack with its license id and the license text", async () => {
    const onBack = vi.fn();
    render(<Licenses onBack={onBack} />);
    expect(await screen.findByText("superpowers")).toBeInTheDocument();
    expect(screen.getByText("Apache-2.0")).toBeInTheDocument();
    expect(screen.getByText("Text of superpowers license")).toBeInTheDocument();
    expect(screen.getByText("Text of anthropic license")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Back to Settings" }));
    expect(onBack).toHaveBeenCalled();
  });

  it("a pack whose license cannot be read still appears", async () => {
    ipc.packLicenses.mockRejectedValue({ kind: "other", message: "x", retryable: false });
    render(<Licenses onBack={vi.fn()} />);
    expect(await screen.findByText("superpowers")).toBeInTheDocument();
  });
});
