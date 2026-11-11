import { describe, expect, it } from "vitest";
import { chooseModel, formatPrice, pickDefaultModel } from "./defaultModel";
import { model } from "./fixtures";

const free = (id: string, context_length: number, supports_tools = true) =>
  model({ id, is_free: true, supports_tools, context_length, prompt_usd_per_mtok: 0, completion_usd_per_mtok: 0 });

describe("pickDefaultModel", () => {
  it("picks the free tool-capable model with the largest context", () => {
    const list = [free("a", 100), free("b", 900), free("c", 500), model({ id: "p", context_length: 99999 })];
    expect(pickDefaultModel(list)?.id).toBe("b");
  });

  it("breaks ties by lowest id", () => {
    expect(pickDefaultModel([free("z", 100), free("m", 100), free("q", 100)])?.id).toBe("m");
  });

  it("falls back to the largest free model when none supports tools", () => {
    const list = [free("a", 100, false), free("b", 700, false)];
    expect(pickDefaultModel(list)?.id).toBe("b");
  });

  it("prefers a tool-capable free model over a larger one without tools", () => {
    expect(pickDefaultModel([free("big", 9000, false), free("small", 10)])?.id).toBe("small");
  });

  it("returns null when nothing is free", () => {
    expect(pickDefaultModel([model({ id: "p" })])).toBeNull();
    expect(pickDefaultModel([])).toBeNull();
  });
});

describe("chooseModel", () => {
  const list = [free("a", 100), free("b", 900)];
  it("uses the first candidate that is in the list", () => {
    expect(chooseModel(list, ["gone", "a"])).toBe("a");
  });
  it("falls back to the default when a remembered model is no longer listed", () => {
    expect(chooseModel(list, ["gone", null])).toBe("b");
  });
  it("is null when nothing matches and there is no default", () => {
    expect(chooseModel([model({ id: "p" })], ["gone"])).toBeNull();
  });
});

describe("formatPrice", () => {
  it("formats paid prices to two decimals", () => {
    expect(formatPrice(model({ id: "p" }))).toBe("$0.50 in, $1.50 out per million tokens");
  });
  it("uses three decimals under one cent", () => {
    expect(formatPrice(model({ id: "p", prompt_usd_per_mtok: 0.006 }))).toBe(
      "$0.006 in, $1.50 out per million tokens",
    );
  });
  it("says Free for free models", () => {
    expect(formatPrice(free("f", 1))).toBe("Free");
  });
});
