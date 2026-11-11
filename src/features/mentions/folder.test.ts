import { beforeEach, describe, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => ({ approvedFolders: vi.fn(), setChatFolder: vi.fn() }));
vi.mock("../../lib/ipc", () => ipc);

import { displayPath, folderName, formatSize, inheritLastFolder } from "./folder";

beforeEach(() => {
  vi.resetAllMocks();
  localStorage.clear();
});

describe("folder helpers", () => {
  it("takes the last path component for both separators and strips \\\\?\\", () => {
    expect(folderName("/home/me/app")).toBe("app");
    expect(folderName("/home/me/app/")).toBe("app");
    expect(folderName("\\\\?\\C:\\Users\\me\\app")).toBe("app");
    expect(displayPath("\\\\?\\C:\\Users\\me\\app")).toBe("C:\\Users\\me\\app");
  });
  it("formats sizes", () => {
    expect(formatSize(340)).toBe("340 B");
    expect(formatSize(1229)).toBe("1.2 KB");
    expect(formatSize(2.1 * 1024 * 1024)).toBe("2.1 MB");
  });
  it("inherits lastFolder only when it is the most recent approved folder", async () => {
    ipc.approvedFolders.mockResolvedValue(["/a", "/b"]);
    ipc.setChatFolder.mockResolvedValue(undefined);
    localStorage.setItem("lastFolder", "/a");
    await inheritLastFolder("n1");
    expect(ipc.setChatFolder).toHaveBeenCalledWith("n1", "/a");
    ipc.setChatFolder.mockClear();
    localStorage.setItem("lastFolder", "/b");
    await inheritLastFolder("n2");
    expect(ipc.setChatFolder).not.toHaveBeenCalled();
  });
  it("ignores failures silently", async () => {
    ipc.approvedFolders.mockResolvedValue(["/a"]);
    ipc.setChatFolder.mockRejectedValue({ kind: "x", message: "gone", retryable: false });
    localStorage.setItem("lastFolder", "/a");
    await expect(inheritLastFolder("n1")).resolves.toBeUndefined();
  });
});
