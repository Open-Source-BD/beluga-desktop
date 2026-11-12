import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import Settings from "./Settings";

const ipc = vi.hoisted(() => ({
  authStatus: vi.fn(), signOut: vi.fn(), listPacks: vi.fn(), packLicenses: vi.fn(),
  spendLimit: vi.fn(), setSpendLimit: vi.fn(), projectRules: vi.fn(), removeProjectRule: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);
vi.mock("../../app/appearance", () => ({ setAppearance: vi.fn(), getAppearance: () => "system" }));

beforeEach(() => {
  vi.resetAllMocks();
  ipc.authStatus.mockResolvedValue({ signed_in: true, storage: "keychain" });
  ipc.spendLimit.mockResolvedValue({ limit_usd: 1, spent_today_usd: 0.254 });
  ipc.setSpendLimit.mockResolvedValue(undefined);
  ipc.projectRules.mockResolvedValue([
    { folder: "/work/shop", rules: ["npm install", "git status"] },
    { folder: "/work/blog", rules: ["pnpm test"] },
  ]);
  ipc.removeProjectRule.mockResolvedValue(undefined);
});
const open = () => render(<Settings onBack={vi.fn()} onSignedOut={vi.fn()} />);

describe("Spending limit for paid models", () => {
  it("shows the limit, today's spend and the fixed sentence", async () => {
    open();
    expect(await screen.findByRole("heading", { name: "Spending limit for paid models" })).toBeInTheDocument();
    expect(await screen.findByText("Spent $0.25 today (UTC day)")).toBeInTheDocument();
    expect(screen.getByLabelText("Limit in dollars")).toHaveValue(1);
    expect(screen.getByLabelText("Limit in dollars")).toHaveAttribute("min", "0.10");
    expect(screen.getByText("Several chats working at once can each go one request past the limit.")).toBeInTheDocument();
    expect(screen.getByText("Tasks on paid models will not pause for cost.")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "No limit" })).toHaveAttribute("aria-checked", "false");
  });

  it("saves a new limit", async () => {
    open();
    const field = await screen.findByLabelText("Limit in dollars");
    await userEvent.clear(field);
    await userEvent.type(field, "2.5");
    await userEvent.click(screen.getByRole("button", { name: "Save limit" }));
    expect(ipc.setSpendLimit).toHaveBeenCalledWith(2.5);
  });

  it("refuses 0.05 with the core's own message", async () => {
    ipc.setSpendLimit.mockRejectedValue({ kind: "other", message: "The limit must be at least $0.10.", retryable: false });
    open();
    const field = await screen.findByLabelText("Limit in dollars");
    await userEvent.clear(field);
    await userEvent.type(field, "0.05");
    await userEvent.click(screen.getByRole("button", { name: "Save limit" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The limit must be at least $0.10.");
    expect(ipc.setSpendLimit).toHaveBeenCalledWith(0.05);
  });

  it("No limit calls setSpendLimit(null) and is shown on when the limit is null", async () => {
    open();
    const sw = await screen.findByRole("switch", { name: "No limit" });
    ipc.spendLimit.mockResolvedValue({ limit_usd: null, spent_today_usd: 0.254 });
    await userEvent.click(sw);
    expect(ipc.setSpendLimit).toHaveBeenCalledWith(null);
    await waitFor(() => expect(screen.getByRole("switch", { name: "No limit" })).toHaveAttribute("aria-checked", "true"));
  });

  it("starts with No limit on when there is none", async () => {
    ipc.spendLimit.mockResolvedValue({ limit_usd: null, spent_today_usd: 0 });
    open();
    await waitFor(() => expect(screen.getByRole("switch", { name: "No limit" })).toHaveAttribute("aria-checked", "true"));
  });
});

describe("Allowed commands", () => {
  it("lists each folder with its rules as text and Remove for each rule", async () => {
    open();
    expect(await screen.findByRole("heading", { name: "Allowed commands" })).toBeInTheDocument();
    expect(await screen.findByText("/work/shop")).toBeInTheDocument();
    expect(screen.getByText("git status")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Remove" })).toHaveLength(3);
    expect(screen.getByText("A saved command runs without asking only in its simple forms.")).toBeInTheDocument();
  });

  it("Remove calls removeProjectRule with the folder and rule, then lists again", async () => {
    open();
    const folder = (await screen.findByText("/work/shop")).closest("li")!;
    ipc.projectRules.mockResolvedValue([{ folder: "/work/blog", rules: ["pnpm test"] }]);
    await userEvent.click(within(folder).getAllByRole("button", { name: "Remove" })[1]);
    expect(ipc.removeProjectRule).toHaveBeenCalledWith("/work/shop", "git status");
    await waitFor(() => expect(screen.queryByText("/work/shop")).toBeNull());
  });

  it("says so when no command is always allowed", async () => {
    ipc.projectRules.mockResolvedValue([]);
    open();
    expect(await screen.findByText("No commands are always allowed.")).toBeInTheDocument();
  });
});
