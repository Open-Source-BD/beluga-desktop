import { describe, expect, it } from "vitest";
import { cheapestPaidTools, formatMoney, formatResetTime, nextReset } from "./allowance";
import { model } from "../models/fixtures";

describe("nextReset", () => {
  it("is the coming 00:00 UTC for a time before midnight", () => {
    expect(nextReset(new Date("2026-11-11T23:59:59.999Z")).toISOString()).toBe("2026-11-12T00:00:00.000Z");
    expect(nextReset(new Date("2026-11-11T05:30:00Z")).toISOString()).toBe("2026-11-12T00:00:00.000Z");
  });
  it("moves to the following day at exactly 00:00 UTC and just after", () => {
    expect(nextReset(new Date("2026-11-12T00:00:00.000Z")).toISOString()).toBe("2026-11-13T00:00:00.000Z");
    expect(nextReset(new Date("2026-11-12T00:00:00.001Z")).toISOString()).toBe("2026-11-13T00:00:00.000Z");
  });
  it("crosses month and year ends", () => {
    expect(nextReset(new Date("2026-12-31T12:00:00Z")).toISOString()).toBe("2027-01-01T00:00:00.000Z");
  });
});

describe("formatResetTime", () => {
  it("shows hours and minutes in the given time zone and locale", () => {
    const d = new Date("2026-11-12T00:00:00Z");
    expect(formatResetTime(d, "en-GB", "Asia/Dhaka")).toBe("06:00");
    expect(formatResetTime(d, "en-GB", "UTC")).toBe("00:00");
  });
});

describe("formatMoney", () => {
  it("uses two decimals and says under $0.01 for tiny amounts", () => {
    expect(formatMoney(0)).toBe("$0.00");
    expect(formatMoney(1.234)).toBe("$1.23");
    expect(formatMoney(0.004)).toBe("under $0.01");
    expect(formatMoney(0.01)).toBe("$0.01");
  });
});

describe("cheapestPaidTools", () => {
  it("returns the three cheapest paid tool-capable models, ties by id", () => {
    const list = [
      model({ id: "z", prompt_usd_per_mtok: 1, completion_usd_per_mtok: 1 }),
      model({ id: "b", prompt_usd_per_mtok: 1, completion_usd_per_mtok: 1 }),
      model({ id: "free", is_free: true, prompt_usd_per_mtok: 0, completion_usd_per_mtok: 0 }),
      model({ id: "notools", supports_tools: false, prompt_usd_per_mtok: 0.01, completion_usd_per_mtok: 0.01 }),
      model({ id: "cheap", prompt_usd_per_mtok: 0.1, completion_usd_per_mtok: 0.1 }),
      model({ id: "pricey", prompt_usd_per_mtok: 9, completion_usd_per_mtok: 9 }),
    ];
    expect(cheapestPaidTools(list).map((m) => m.id)).toEqual(["cheap", "b", "z"]);
  });
});
