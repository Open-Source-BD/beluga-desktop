import { describe, expect, it } from "vitest";
import { shouldFollow } from "./follow";

describe("shouldFollow", () => {
  it("follows at the bottom and near it", () => {
    expect(shouldFollow(600, 400, 1000)).toBe(true);
    expect(shouldFollow(570, 400, 1000)).toBe(true);
  });
  it("does not follow once scrolled up", () => {
    expect(shouldFollow(100, 400, 1000)).toBe(false);
  });
});
