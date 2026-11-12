import { describe, expect, it } from "vitest";
import { atEnd } from "./measure";

const box = (scrollHeight: number, clientHeight: number, scrollTop: number) =>
  ({ scrollHeight, clientHeight, scrollTop }) as unknown as HTMLElement;

describe("atEnd", () => {
  it("allows four pixels of slack, as fractional display scaling leaves", () => {
    // 1000 tall in a 400 box: the end is at 600.
    expect(atEnd(box(1000, 400, 600))).toBe(true);
    expect(atEnd(box(1000, 400, 596))).toBe(true);
    expect(atEnd(box(1000, 400, 597.5))).toBe(true);
    expect(atEnd(box(1000.4, 400, 596.4))).toBe(true);
    expect(atEnd(box(1000, 400, 595.9))).toBe(false);
    expect(atEnd(box(1000, 400, 0))).toBe(false);
  });

  it("a box whose content fits, or nearly fits, is at its end", () => {
    expect(atEnd(box(400, 400, 0))).toBe(true);
    expect(atEnd(box(404, 400, 0))).toBe(true);
    expect(atEnd(box(405, 400, 0))).toBe(false);
  });
});
