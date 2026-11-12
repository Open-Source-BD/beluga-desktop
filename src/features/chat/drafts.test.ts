import { afterEach, describe, expect, it, vi } from "vitest";
import { clearDraft, loadDraft, saveDraft } from "./drafts";

afterEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
});

describe("drafts", () => {
  it("saves and loads per chat under draft:{id}, and draft:new for a new chat", () => {
    saveDraft("c1", "hello");
    saveDraft(null, "fresh");
    expect(localStorage.getItem("draft:c1")).toBe("hello");
    expect(localStorage.getItem("draft:new")).toBe("fresh");
    expect(loadDraft("c1")).toBe("hello");
    expect(loadDraft(null)).toBe("fresh");
    expect(loadDraft("other")).toBe("");
  });

  it("clears a draft", () => {
    saveDraft("c1", "hello");
    clearDraft("c1");
    expect(loadDraft("c1")).toBe("");
    expect(localStorage.getItem("draft:c1")).toBeNull();
  });

  it("ignores storage errors", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "removeItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(loadDraft("c1")).toBe("");
    expect(() => saveDraft("c1", "x")).not.toThrow();
    expect(() => clearDraft("c1")).not.toThrow();
  });
});
