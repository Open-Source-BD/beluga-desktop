import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PackView, SkillView } from "../../lib/ipc";
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
  listPacks: vi.fn(),
  chatState: vi.fn(),
  activateSkill: vi.fn(),
  deactivateSkill: vi.fn(),
  compactChat: vi.fn(),
  exportChatWithDialog: vi.fn(),
  getSkill: vi.fn(),
  skillFile: vi.fn(),
  setSkillOn: vi.fn(),
  setPackOn: vi.fn(),
  extraPacks: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);

const PAID = model({ id: "test/model", name: "Paid One" });
const CHAT = { id: "c1", title: "Login page", model_id: "test/model", updated_at: 1 };
const EMPTY_STATE = { active_skills: [], folder: null, compacted: false };

const skill = (id: string, over: Partial<SkillView> = {}): SkillView => ({
  id, name: id.split(":").pop()!, description: `About ${id}`, on: true, acts_on_computer: false,
  broken: null, body_chars: 10, available: true, ...over,
});
const pack = (id: string, title: string, skills: SkillView[], origin: PackView["origin"] = "built_in"): PackView => ({
  id, title, origin, license: "MIT", repo: "", commit: "", about: "", on: true, error: null, skills,
});

const box = () => screen.findByLabelText("Message");

beforeEach(() => {
  vi.resetAllMocks();
  localStorage.setItem("lastModel", "test/model");
  ipc.listChats.mockResolvedValue([]);
  ipc.listModels.mockResolvedValue([PAID]);
  ipc.getMessages.mockResolvedValue([]);
  ipc.keyStatus.mockResolvedValue({ is_free_tier: false, free_daily: null, usage_usd: 0, limit_remaining_usd: 5 });
  ipc.monthSpend.mockResolvedValue(1.5);
  ipc.chatUsage.mockResolvedValue({ requests: 7, cost_usd: 0.25 });
  ipc.createChat.mockResolvedValue(CHAT);
  ipc.chatState.mockResolvedValue(EMPTY_STATE);
  ipc.listPacks.mockResolvedValue([
    pack("superpowers", "superpowers", [skill("superpowers:brainstorming"), skill("superpowers:off", { available: false, on: false })]),
  ]);
  ipc.getSkill.mockResolvedValue({
    skill: skill("superpowers:brainstorming"),
    pack_id: "superpowers",
    pack_title: "superpowers",
    license: "MIT",
    source_url: "https://github.com/obra/superpowers",
    body: "Think first.",
    files: ["SKILL.md", "notes.md"],
  });
  ipc.activateSkill.mockResolvedValue(EMPTY_STATE);
  ipc.skillFile.mockResolvedValue({ name: "notes.md", mime: "text/markdown", data_base64: "aGk=" });
});

describe("Shell skills screen", () => {
  it("the sidebar row shows how many are on and opens the screen", async () => {
    render(<Shell />);
    expect(await screen.findByText("1 on")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /^Skills/ }));
    expect(await screen.findByRole("heading", { name: "Skills" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Back to chat" }));
    expect(await box()).toBeInTheDocument();
  });

  it("/skills opens the screen", async () => {
    render(<Shell />);
    await userEvent.type(await box(), "/skills{Enter}");
    expect(await screen.findByRole("heading", { name: "Skills" })).toBeInTheDocument();
  });

  it("Use in this chat creates a chat, activates the skill and returns to the chat", async () => {
    render(<Shell />);
    await userEvent.click(await screen.findByRole("button", { name: /^Skills/ }));
    await userEvent.click(await screen.findByRole("button", { name: "brainstorming" }));
    await userEvent.click(await screen.findByRole("button", { name: "Use in this chat" }));
    await waitFor(() => expect(ipc.activateSkill).toHaveBeenCalledWith("c1", "superpowers:brainstorming", "test/model"));
    expect(await box()).toBeInTheDocument();
  });

  it("Attach to this chat adds the file as a chip in the chat", async () => {
    render(<Shell />);
    await userEvent.click(await screen.findByRole("button", { name: /^Skills/ }));
    await userEvent.click(await screen.findByRole("button", { name: "brainstorming" }));
    await userEvent.click(await screen.findByRole("button", { name: "Attach to this chat" }));
    await waitFor(() => expect(ipc.skillFile).toHaveBeenCalledWith("superpowers:brainstorming", "notes.md"));
    expect(await screen.findByText("notes.md")).toBeInTheDocument();
  });
});
