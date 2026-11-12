import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { APP_NAME } from "../../app/brand";
import type { ExtraPack, PackView, SkillView } from "../../lib/ipc";
import SkillsScreen from "./SkillsScreen";

const ipc = vi.hoisted(() => ({
  listPacks: vi.fn(),
  getSkill: vi.fn(),
  setSkillOn: vi.fn(),
  setPackOn: vi.fn(),
  extraPacks: vi.fn(),
  installExtraPack: vi.fn(),
  removePack: vi.fn(),
  previewAdd: vi.fn(),
  confirmAdd: vi.fn(),
  cancelAdd: vi.fn(),
  removeSkill: vi.fn(),
  openExternal: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ipc);

const skill = (id: string, over: Partial<SkillView> = {}): SkillView => ({
  id,
  name: id.split(":").pop()!,
  description: `About ${id}`,
  on: true,
  acts_on_computer: false,
  broken: null,
  body_chars: 10,
  available: true,
  ...over,
});
const pack = (id: string, skills: SkillView[], over: Partial<PackView> = {}): PackView => ({
  id,
  title: id,
  origin: "built_in",
  license: "MIT",
  repo: "",
  commit: "",
  about: "",
  on: true,
  error: null,
  skills,
  ...over,
});
const sp = pack("superpowers", [
  skill("superpowers:alpha"),
  skill("superpowers:beta"),
  skill("superpowers:gamma", { on: false, available: false }),
  skill("superpowers:delta"),
  skill("superpowers:epsilon"),
]);
const mine = pack("yours", [skill("review-style", { description: "How I review" })], {
  title: "Added by you",
  origin: "yours",
  license: "",
});
const RED: ExtraPack = {
  id: "claude-red",
  title: "claude-red",
  about: "Offensive security",
  license: "MIT",
  repo: "SnailSploit/Claude-Red",
  installed: false,
  needs_warning: true,
};
const VERCEL: ExtraPack = { ...RED, id: "vercel", title: "vercel", about: "Vercel skills", installed: true, needs_warning: false };

const setup = (over: Partial<Parameters<typeof SkillsScreen>[0]> = {}) => {
  const p = { onBack: vi.fn(), onChanged: vi.fn(), onUse: vi.fn().mockResolvedValue(undefined), onAttach: vi.fn().mockResolvedValue(undefined), ...over };
  render(<SkillsScreen {...p} />);
  return p;
};

beforeEach(() => {
  vi.resetAllMocks();
  ipc.listPacks.mockResolvedValue([sp, mine]);
  ipc.extraPacks.mockResolvedValue([RED, VERCEL]);
  ipc.setSkillOn.mockResolvedValue(undefined);
  ipc.setPackOn.mockResolvedValue(undefined);
});

describe("SkillsScreen installed", () => {
  it("lede shows how many are on, using APP_NAME", async () => {
    setup();
    expect(
      await screen.findByText(
        `Skills are instructions ${APP_NAME} follows for a kind of task. 5 are on. Fewer is better on small free models.`,
      ),
    ).toBeInTheDocument();
  });

  it("a pack card shows title, counts, license and origin; Show all expands", async () => {
    setup();
    expect(await screen.findByText("5 skills, 4 on")).toBeInTheDocument();
    expect(screen.getByText(/MIT/)).toBeInTheDocument();
    expect(screen.getByText(/built in/)).toBeInTheDocument();
    expect(screen.getByText(/added by you/)).toBeInTheDocument();
    expect(screen.getByText("alpha")).toBeInTheDocument();
    expect(screen.queryByText("delta")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Show all 5" }));
    expect(screen.getByText("delta")).toBeInTheDocument();
  });

  it("toggling a skill calls setSkillOn and updates the count", async () => {
    const p = setup();
    await userEvent.click(await screen.findByRole("switch", { name: "alpha" }));
    expect(ipc.setSkillOn).toHaveBeenCalledWith("superpowers:alpha", false);
    expect(await screen.findByText(/4 are on/)).toBeInTheDocument();
    await waitFor(() => expect(p.onChanged).toHaveBeenCalled());
  });

  it("a failed toggle reverts and alerts", async () => {
    ipc.setSkillOn.mockRejectedValue({ kind: "other", message: "Could not save.", retryable: false });
    setup();
    const sw = await screen.findByRole("switch", { name: "alpha" });
    await userEvent.click(sw);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save.");
    expect(screen.getByRole("switch", { name: "alpha" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByText(/5 are on/)).toBeInTheDocument();
  });

  it("toggling a pack calls setPackOn and greys its skills without changing their switches", async () => {
    setup();
    await userEvent.click(await screen.findByRole("switch", { name: "superpowers pack" }));
    expect(ipc.setPackOn).toHaveBeenCalledWith("superpowers", false);
    expect(await screen.findByText(/1 are on/)).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "alpha" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("switch", { name: "alpha" }).closest("li")).toHaveClass("dim");
  });

  it("search filters across packs by name and description and expands matches", async () => {
    setup();
    await screen.findByText("alpha");
    await userEvent.type(screen.getByRole("searchbox", { name: "Search skills" }), "epsilon");
    expect(screen.getByText("epsilon")).toBeInTheDocument();
    expect(screen.queryByText("alpha")).not.toBeInTheDocument();
    expect(screen.queryByText("review-style")).not.toBeInTheDocument();
    await userEvent.clear(screen.getByRole("searchbox"));
    await userEvent.type(screen.getByRole("searchbox"), "how i review");
    expect(screen.getByText("review-style")).toBeInTheDocument();
  });

  it("no match shows No skills match.", async () => {
    setup();
    await screen.findByText("alpha");
    await userEvent.type(screen.getByRole("searchbox"), "zzzz");
    expect(screen.getByText("No skills match.")).toBeInTheDocument();
  });

  it("a broken skill shows its reason and has no switch", async () => {
    ipc.listPacks.mockResolvedValue([pack("p", [skill("p:bad", { broken: "No description found.", available: false })])]);
    setup();
    expect(await screen.findByText("No description found.")).toBeInTheDocument();
    expect(screen.queryByRole("switch", { name: "bad" })).not.toBeInTheDocument();
  });
});

describe("SkillsScreen detail", () => {
  const detail = (over: Partial<SkillView> = {}, pack_id = "superpowers") => ({
    skill: skill("superpowers:alpha", over),
    pack_id,
    pack_title: pack_id,
    license: "MIT",
    source_url: "https://github.com/obra/superpowers/tree/abc",
    body: "# Alpha\n\nDo the thing.",
    files: ["SKILL.md", "notes.md"],
  });
  const open = async () => {
    await userEvent.click(await screen.findByRole("button", { name: "alpha" }));
    await screen.findByText("Do the thing.");
  };

  it("shows the acts note only when acts_on_computer", async () => {
    const note = "Written for an agent that can edit files and run commands. Open a folder in the chat and it can do that, asking you first.";
    ipc.getSkill.mockResolvedValue(detail({ acts_on_computer: true }));
    setup();
    await open();
    expect(screen.getByText(note)).toBeInTheDocument();
  });

  it("omits the acts note otherwise; View source uses the GitHub-only open", async () => {
    ipc.getSkill.mockResolvedValue(detail());
    setup();
    await open();
    expect(screen.queryByText(/Written for an agent/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "View source" }));
    expect(ipc.openExternal).toHaveBeenCalledWith("https://github.com/obra/superpowers/tree/abc");
  });

  it("Use in this chat calls onUse; Attach calls onAttach with the file", async () => {
    ipc.getSkill.mockResolvedValue(detail());
    const p = setup();
    await open();
    await userEvent.click(screen.getByRole("button", { name: "Use in this chat" }));
    expect(p.onUse).toHaveBeenCalledWith("superpowers:alpha");
    await userEvent.click(screen.getByRole("button", { name: "Attach to this chat" }));
    expect(p.onAttach).toHaveBeenCalledWith("superpowers:alpha", "notes.md");
  });

  it("a rejected use shows the message", async () => {
    ipc.getSkill.mockResolvedValue(detail());
    setup({ onUse: vi.fn().mockRejectedValue({ kind: "other", message: "Too long for this model.", retryable: false }) });
    await open();
    await userEvent.click(screen.getByRole("button", { name: "Use in this chat" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Too long for this model.");
  });

  it("Remove is offered for the user's own skill only", async () => {
    ipc.getSkill.mockResolvedValue(detail());
    ipc.removeSkill.mockResolvedValue(undefined);
    setup();
    await open();
    expect(screen.queryByRole("button", { name: "Remove" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Back to Skills" }));
    ipc.getSkill.mockResolvedValue({ ...detail({}, "yours"), skill: skill("review-style") });
    await userEvent.click(screen.getByRole("button", { name: "review-style" }));
    await userEvent.click(await screen.findByRole("button", { name: "Remove" }));
    expect(ipc.removeSkill).toHaveBeenCalledWith("review-style");
  });
});

describe("SkillsScreen add", () => {
  const preview = { token: "t1", skills: [skill("one"), skill("two")], skipped: ["run.sh"] };

  it.each([
    ["A folder on this computer", { kind: "folder" }],
    ["A ZIP file", { kind: "zip" }],
  ])("%s calls previewAdd, lists found and skipped, then confirmAdd", async (label, source) => {
    ipc.previewAdd.mockResolvedValue(preview);
    ipc.confirmAdd.mockResolvedValue(mine);
    const p = setup();
    await userEvent.click(await screen.findByRole("button", { name: "Add a skill" }));
    await userEvent.click(screen.getByRole("button", { name: label }));
    expect(ipc.previewAdd).toHaveBeenCalledWith(source);
    expect(await screen.findByText("one")).toBeInTheDocument();
    expect(screen.getByText("run.sh")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Add 2 skills" }));
    expect(ipc.confirmAdd).toHaveBeenCalledWith("t1");
    await waitFor(() => expect(p.onChanged).toHaveBeenCalled());
  });

  it("A GitHub link sends the typed URL; one skill reads 'Add 1 skill'", async () => {
    ipc.previewAdd.mockResolvedValue({ token: "t2", skills: [skill("one")], skipped: [] });
    setup();
    await userEvent.click(await screen.findByRole("button", { name: "Add a skill" }));
    await userEvent.click(screen.getByRole("button", { name: "A GitHub link" }));
    await userEvent.type(screen.getByLabelText("GitHub link"), "https://github.com/o/r");
    await userEvent.click(screen.getByRole("button", { name: "Find skills" }));
    expect(ipc.previewAdd).toHaveBeenCalledWith({ kind: "github", url: "https://github.com/o/r" });
    expect(await screen.findByRole("button", { name: "Add 1 skill" })).toBeInTheDocument();
  });

  it("a cancelled dialog returns to the choices", async () => {
    ipc.previewAdd.mockResolvedValue(null);
    setup();
    await userEvent.click(await screen.findByRole("button", { name: "Add a skill" }));
    await userEvent.click(screen.getByRole("button", { name: "A ZIP file" }));
    expect(await screen.findByRole("button", { name: "A ZIP file" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("a rejected preview shows the core's exact message", async () => {
    ipc.previewAdd.mockRejectedValue({ kind: "bad", message: "No SKILL.md was found there.", retryable: false });
    setup();
    await userEvent.click(await screen.findByRole("button", { name: "Add a skill" }));
    await userEvent.click(screen.getByRole("button", { name: "A folder on this computer" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("No SKILL.md was found there.");
  });

  it("backing out after a preview calls cancelAdd", async () => {
    ipc.previewAdd.mockResolvedValue(preview);
    ipc.cancelAdd.mockResolvedValue(undefined);
    setup();
    await userEvent.click(await screen.findByRole("button", { name: "Add a skill" }));
    await userEvent.click(screen.getByRole("button", { name: "A ZIP file" }));
    await screen.findByText("one");
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(ipc.cancelAdd).toHaveBeenCalledWith("t1");
  });
});

describe("SkillsScreen more packs", () => {
  const goMore = async (text = "Offensive security") => {
    await userEvent.click(await screen.findByRole("tab", { name: "More packs" }));
    await screen.findByText(text);
  };

  it("installing claude-red opens the warning first; Cancel installs nothing", async () => {
    setup();
    await goMore();
    await userEvent.click(within(screen.getByText("Offensive security").closest("li")!).getByRole("button", { name: "Install" }));
    const dlg = screen.getByRole("dialog");
    expect(within(dlg).getByText("This pack is for authorised security testing")).toBeInTheDocument();
    expect(
      within(dlg).getByText(
        "Claude-Red contains offensive security techniques. Use it only on systems you own or have written permission to test. Many models will refuse parts of it. It installs switched off.",
      ),
    ).toBeInTheDocument();
    await userEvent.click(within(dlg).getByRole("button", { name: "Cancel" }));
    expect(ipc.installExtraPack).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("Install, switched off calls installExtraPack with acknowledged true", async () => {
    ipc.installExtraPack.mockResolvedValue(pack("claude-red", []));
    const p = setup();
    await goMore();
    await userEvent.click(within(screen.getByText("Offensive security").closest("li")!).getByRole("button", { name: "Install" }));
    await userEvent.click(screen.getByRole("button", { name: "Install, switched off" }));
    expect(ipc.installExtraPack).toHaveBeenCalledWith("claude-red", true);
    await waitFor(() => expect(p.onChanged).toHaveBeenCalled());
  });

  it("a pack without a warning installs with acknowledged false", async () => {
    ipc.extraPacks.mockResolvedValue([{ ...VERCEL, installed: false }]);
    ipc.installExtraPack.mockResolvedValue(pack("vercel", []));
    setup();
    await goMore("Vercel skills");
    await userEvent.click(screen.getByRole("button", { name: "Install" }));
    expect(ipc.installExtraPack).toHaveBeenCalledWith("vercel", false);
  });

  it("installed extra packs show Remove", async () => {
    ipc.removePack.mockResolvedValue(undefined);
    setup();
    await goMore();
    await userEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(ipc.removePack).toHaveBeenCalledWith("vercel");
  });

  it("built-in packs never show Remove on the Installed tab", async () => {
    setup();
    await screen.findByText("alpha");
    expect(screen.queryByRole("button", { name: "Remove" })).not.toBeInTheDocument();
  });
});
