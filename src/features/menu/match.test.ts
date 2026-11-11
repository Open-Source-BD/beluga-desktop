import { describe, expect, it } from "vitest";
import { matchRows, type Row } from "./match";

const row = (name: string, description = "", group: Row["group"] = "Skills"): Row => ({
  id: name,
  name,
  description,
  source: "x",
  group,
});

const names = (rows: Row[]) => rows.map((r) => r.name);

describe("matchRows", () => {
  const rows = [
    row("/superpowers:brainstorming", "Work out what to build"),
    row("/superpowers:writing-plans", "Turn a design into steps"),
    row("/zeta", "Make a plan for the week"),
    row("/my_notes", "Quick notes"),
  ];

  it("returns every row, in order, for an empty query", () => {
    expect(names(matchRows("", rows))).toEqual(names(rows));
  });

  it("finds a skill by the start of a word in its name", () => {
    expect(names(matchRows("br", rows))).toEqual(["/superpowers:brainstorming"]);
  });

  it("ranks a name match above a description-only match", () => {
    expect(names(matchRows("plan", rows))).toEqual(["/superpowers:writing-plans", "/zeta"]);
  });

  it("splits words on colon, hyphen and underscore", () => {
    expect(names(matchRows("wri", rows))).toEqual(["/superpowers:writing-plans"]);
    expect(names(matchRows("notes", rows))).toEqual(["/my_notes"]);
    expect(names(matchRows("plans", rows))).toEqual(["/superpowers:writing-plans"]);
  });

  it("ignores case and a leading slash, and keeps original order within a rank", () => {
    expect(names(matchRows("/SUPER", rows))).toEqual([
      "/superpowers:brainstorming",
      "/superpowers:writing-plans",
    ]);
  });

  it("returns nothing when nothing matches", () => {
    expect(matchRows("qqq", rows)).toEqual([]);
  });
});
