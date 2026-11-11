import { describe, expect, it } from "vitest";
import type { UiError } from "../../lib/ipc";
import { nextStep } from "./errors";

const err = (kind: UiError["kind"], retryable = false): UiError => ({
  kind,
  message: "m",
  retryable,
});

describe("nextStep", () => {
  it("maps each kind to its step", () => {
    expect(nextStep(err("unauthorized"))).toEqual({ label: "Sign in again", action: "sign_in" });
    for (const k of ["no_credit", "model_unavailable", "context_too_long"] as const) {
      expect(nextStep(err(k))).toEqual({ label: "Pick another model", action: "pick_model" });
    }
    for (const k of ["rate_limited", "network"] as const) {
      expect(nextStep(err(k))).toEqual({ label: "Retry", action: "retry" });
    }
  });

  it("offers Retry for other only when retryable", () => {
    expect(nextStep(err("other", true))).toEqual({ label: "Retry", action: "retry" });
    expect(nextStep(err("other", false))).toBeNull();
  });
});
