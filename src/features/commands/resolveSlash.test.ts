import { describe, expect, it } from "vitest";
import type { Row } from "../menu/match";
import { resolveSlash } from "./resolveSlash";

const skills: Row[] = [
  { id: "superpowers:brainstorming", name: "/superpowers:brainstorming", description: "", source: "x", group: "Skills" },
  { id: "my-style", name: "/my-style", description: "", source: "x", group: "Skills" },
];

describe("resolveSlash", () => {
  it("knows an exact skill id, with and without text", () => {
    expect(resolveSlash("/my-style", skills)).toEqual({ kind: "skill", id: "my-style", rest: "" });
    expect(resolveSlash("/superpowers:brainstorming  a login page", skills)).toEqual({
      kind: "skill", id: "superpowers:brainstorming", rest: "a login page",
    });
  });
  it("knows a command, in any case, with text", () => {
    expect(resolveSlash("/Compact keep the database decisions", skills)).toEqual({
      kind: "command", name: "/compact", rest: "keep the database decisions",
    });
  });
  it("is case-sensitive for skill ids", () => {
    expect(resolveSlash("/My-Style", skills)).toEqual({ kind: "plain" });
  });
  it("treats anything else as plain text", () => {
    for (const t of ["/usr/local/bin", "/etc", "/superpowers:brain", "/", " /new", "hello /new"]) {
      expect(resolveSlash(t, skills)).toEqual({ kind: "plain" });
    }
  });
});
