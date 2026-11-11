import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useRef } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import Composer, { type ComposerControl } from "../chat/Composer";
import { useMentionMenu } from "./useMentionMenu";

const ipc = vi.hoisted(() => ({ folderFiles: vi.fn(), attachFolderFile: vi.fn() }));
vi.mock("../../lib/ipc", () => ipc);

const ENTRIES = [
  { path: "src/routes/login.ts", is_dir: false, bytes: 1229 },
  { path: "src/routes", is_dir: true, bytes: 0 },
  { path: "README.md", is_dir: false, bytes: 340 },
];
const list = (over = {}) => ({ entries: ENTRIES, total: 3, cut: false, ...over });
const ATTACHMENT = { name: "login.ts", mime: "text/plain", data: "x", size: 1229 };

function Harness({
  folder = "/home/me/app" as string | null,
  openFolder = vi.fn().mockResolvedValue(true),
  acceptsImages = true,
}) {
  const control = useRef<ComposerControl>(null);
  const m = useMentionMenu({ chatId: "c1", folder, openFolder, control });
  return (
    <Composer
      controlRef={control}
      menu={m.menu}
      fieldAria={m.fieldAria}
      onKeyIntercept={m.onKeyIntercept}
      onTextChange={m.onTextChange}
      onSend={vi.fn()}
      acceptsImages={acceptsImages}
    />
  );
}

const box = () => screen.getByLabelText("Message") as HTMLTextAreaElement;

beforeEach(() => {
  vi.resetAllMocks();
  ipc.folderFiles.mockResolvedValue(list());
});

describe("useMentionMenu", () => {
  it("opens for @ at the start and after a space, but not for a@b", async () => {
    render(<Harness />);
    await userEvent.type(box(), "@");
    expect(await screen.findByRole("listbox")).toBeInTheDocument();
    await userEvent.clear(box());
    await userEvent.type(box(), "see @");
    expect(await screen.findByRole("listbox")).toBeInTheDocument();
    await userEvent.clear(box());
    await userEvent.type(box(), "a@b");
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(ipc.folderFiles).toHaveBeenCalledTimes(2);
  });

  it("closes on whitespace and on Escape", async () => {
    render(<Harness />);
    await userEvent.type(box(), "@ro");
    await screen.findByRole("listbox");
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).toBeNull();
    await userEvent.clear(box());
    await userEvent.type(box(), "@ro");
    await screen.findByRole("listbox");
    await userEvent.keyboard(" ");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("queries folderFiles with the typed text, debounced", async () => {
    render(<Harness />);
    await userEvent.type(box(), "@rou");
    expect(ipc.folderFiles).not.toHaveBeenCalled();
    await screen.findByRole("listbox");
    expect(ipc.folderFiles).toHaveBeenCalledTimes(1);
    expect(ipc.folderFiles).toHaveBeenCalledWith("c1", "rou");
  });

  it("lists the folder's top entries for a bare @ and shows name, folder, size", async () => {
    render(<Harness />);
    await userEvent.type(box(), "@");
    const opts = await screen.findAllByRole("option");
    expect(ipc.folderFiles).toHaveBeenCalledWith("c1", "");
    expect(opts).toHaveLength(3);
    expect(within(opts[0]).getByText("login.ts")).toBeInTheDocument();
    expect(within(opts[0]).getByText("src/routes/")).toBeInTheDocument();
    expect(within(opts[0]).getByText("1.2 KB")).toBeInTheDocument();
    expect(within(opts[1]).getByText("folder, lists its files")).toBeInTheDocument();
    expect(within(opts[2]).getByText("340 B")).toBeInTheDocument();
  });

  it("shows the header with folder name, query and counts, and the cut note", async () => {
    ipc.folderFiles.mockResolvedValue(list({ total: 9000, cut: true }));
    render(<Harness />);
    await userEvent.type(box(), "@rou");
    await screen.findByRole("listbox");
    expect(screen.getByText('Files in app matching "rou"')).toBeInTheDocument();
    expect(screen.getByText("3 of 9000")).toBeInTheDocument();
    expect(screen.getByText("list cut at 5,000 files")).toBeInTheDocument();
  });

  it("ignores a stale response that arrives after a newer one", async () => {
    let first!: (v: unknown) => void;
    ipc.folderFiles
      .mockReturnValueOnce(new Promise((r) => (first = r)))
      .mockResolvedValueOnce(list({ entries: [{ path: "newer.ts", is_dir: false, bytes: 1 }], total: 1 }));
    render(<Harness />);
    await userEvent.type(box(), "@a");
    await waitFor(() => expect(ipc.folderFiles).toHaveBeenCalledTimes(1));
    await userEvent.type(box(), "b");
    await screen.findByText("newer.ts");
    first(list({ entries: [{ path: "older.ts", is_dir: false, bytes: 1 }], total: 1 }));
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.queryByText("older.ts")).toBeNull();
    expect(screen.getByText("newer.ts")).toBeInTheDocument();
  });

  it("Enter attaches through attachFolderFile, adds a chip, and removes the @query", async () => {
    ipc.attachFolderFile.mockResolvedValue(ATTACHMENT);
    render(<Harness />);
    await userEvent.type(box(), "look at @rou");
    await screen.findByRole("listbox");
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(screen.getByRole("list", { name: "Attachments" })).toBeInTheDocument());
    expect(ipc.attachFolderFile).toHaveBeenCalledWith("c1", "src/routes/login.ts");
    expect(within(screen.getByRole("list", { name: "Attachments" })).getByText("login.ts")).toBeInTheDocument();
    expect(box().value).toBe("look at");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("keeps the rest of the text, one space, caret where the token was", async () => {
    ipc.attachFolderFile.mockResolvedValue(ATTACHMENT);
    render(<Harness />);
    await userEvent.type(box(), "look at @rou now");
    expect(screen.queryByRole("listbox")).toBeNull();
    await userEvent.keyboard("{ArrowLeft>4/}");
    await screen.findByRole("listbox");
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(box().value).toBe("look at now"));
    expect(box().selectionStart).toBe(8);
  });

  it("shows the core's message as an alert and leaves text and chips alone on refusal", async () => {
    ipc.attachFolderFile.mockRejectedValue({ kind: "bad", message: "That file is too large.", retryable: false });
    render(<Harness />);
    await userEvent.type(box(), "@rou");
    await screen.findByRole("listbox");
    await userEvent.keyboard("{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent("That file is too large.");
    expect(box().value).toBe("@rou");
    expect(screen.queryByRole("list", { name: "Attachments" })).toBeNull();
  });

  it("with no folder shows one row that opens the folder dialog and reopens the menu", async () => {
    const openFolder = vi.fn().mockResolvedValue(true);
    const { rerender } = render(<Harness folder={null} openFolder={openFolder} />);
    await userEvent.type(box(), "@");
    const opts = await screen.findAllByRole("option");
    expect(opts).toHaveLength(1);
    expect(opts[0]).toHaveTextContent("Open a folder to mention its files");
    expect(ipc.folderFiles).not.toHaveBeenCalled();
    await userEvent.click(opts[0]);
    expect(openFolder).toHaveBeenCalledTimes(1);
    rerender(<Harness folder="/home/me/app" openFolder={openFolder} />);
    expect(await screen.findByText("login.ts")).toBeInTheDocument();
    expect(box().value).toBe("@");
  });

  it("does nothing when the folder dialog is cancelled", async () => {
    const openFolder = vi.fn().mockResolvedValue(false);
    render(<Harness folder={null} openFolder={openFolder} />);
    await userEvent.type(box(), "@");
    await userEvent.click(await screen.findByRole("option"));
    expect(openFolder).toHaveBeenCalled();
    expect(box().value).toBe("@");
  });

  it("renders names as plain text", async () => {
    ipc.folderFiles.mockResolvedValue(list({ entries: [{ path: "<img src=x onerror=alert(1)>.md", is_dir: false, bytes: 5 }], total: 1 }));
    render(<Harness />);
    await userEvent.type(box(), "@");
    expect(await screen.findByText("<img src=x onerror=alert(1)>.md")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("a sixth file shows the limit message and leaves the text and caret alone", async () => {
    let n = 0;
    ipc.attachFolderFile.mockImplementation(async () => ({ ...ATTACHMENT, name: `f${++n}.ts` }));
    render(<Harness />);
    for (let i = 0; i < 5; i++) {
      await userEvent.type(box(), "@");
      await screen.findByText("login.ts");
      await userEvent.keyboard("{Enter}");
      await waitFor(() => expect(box().value).toBe(""));
    }
    await userEvent.type(box(), "@");
    await screen.findByText("login.ts");
    await userEvent.keyboard("{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent("You can attach up to 5 files.");
    expect(box().value).toBe("@");
    expect(box().selectionStart).toBe(1);
  });

  it("an image for a model that cannot read images is refused and the text stays", async () => {
    ipc.attachFolderFile.mockResolvedValue({ name: "a.png", mime: "image/png", data: "x", size: 1 });
    render(<Harness acceptsImages={false} />);
    await userEvent.type(box(), "see @a");
    await screen.findByText("login.ts");
    await userEvent.keyboard("{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent("This model cannot read images.");
    expect(box().value).toBe("see @a");
    expect(screen.queryByRole("list", { name: "Attachments" })).toBeNull();
  });

  it("a second Enter before the attach resolves does nothing", async () => {
    let done!: (v: unknown) => void;
    ipc.attachFolderFile.mockReturnValue(new Promise((r) => (done = r)));
    render(<Harness />);
    await userEvent.type(box(), "@");
    await screen.findByText("login.ts");
    await userEvent.keyboard("{Enter}{Enter}");
    done(ATTACHMENT);
    await waitFor(() => expect(box().value).toBe(""));
    expect(ipc.attachFolderFile).toHaveBeenCalledTimes(1);
    expect(within(screen.getByRole("list", { name: "Attachments" })).getAllByRole("listitem")).toHaveLength(1);
  });

  it("Enter right after a new character, before new results arrive, attaches nothing", async () => {
    render(<Harness />);
    await userEvent.type(box(), "@a");
    await screen.findByText("login.ts");
    await userEvent.keyboard("b{Enter}");
    expect(ipc.attachFolderFile).not.toHaveBeenCalled();
    expect(box().value).toBe("@ab");
  });

  it("removes the whole token when the caret is in the middle of it", async () => {
    ipc.attachFolderFile.mockResolvedValue(ATTACHMENT);
    render(<Harness />);
    await userEvent.type(box(), "@rou");
    await screen.findByText("login.ts");
    await userEvent.keyboard("{ArrowLeft}{ArrowLeft}");
    await waitFor(() => expect(ipc.folderFiles).toHaveBeenLastCalledWith("c1", "r"));
    await screen.findByText("login.ts");
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(box().value).toBe(""));
  });
});
