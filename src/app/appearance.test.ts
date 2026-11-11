import { beforeEach, describe, expect, it } from "vitest";
import { applyAppearance, getAppearance, setAppearance } from "./appearance";

describe("appearance", () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.removeAttribute("data-theme");
  });
  it("defaults to system", () => {
    expect(getAppearance()).toBe("system");
  });
  it("system removes the attribute", () => {
    document.documentElement.setAttribute("data-theme", "dark");
    applyAppearance("system");
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
  });
  it("dark sets the attribute and persists", () => {
    setAppearance("dark");
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(localStorage.getItem("appearance")).toBe("dark");
  });
  it("ignores a corrupt stored value", () => {
    localStorage.setItem("appearance", "neon");
    expect(getAppearance()).toBe("system");
  });
});
