import type { UiError } from "../../lib/ipc";

export type NextStepAction = "sign_in" | "pick_model" | "retry";

export interface NextStep {
  label: string;
  action: NextStepAction;
}

/** The one button an error offers, or null when there is nothing the user can do. */
export function nextStep(error: UiError): NextStep | null {
  switch (error.kind) {
    case "unauthorized":
      return { label: "Sign in again", action: "sign_in" };
    case "no_credit":
    case "model_unavailable":
    case "context_too_long":
      return { label: "Pick another model", action: "pick_model" };
    case "rate_limited":
    case "network":
      return { label: "Retry", action: "retry" };
    default:
      return error.retryable ? { label: "Retry", action: "retry" } : null;
  }
}

/** Command rejections are UiError values; anything else becomes a generic one. */
export function toUiError(value: unknown): UiError {
  if (
    typeof value === "object" &&
    value !== null &&
    "kind" in value &&
    "message" in value
  ) {
    return value as UiError;
  }
  return {
    kind: "other",
    message: "Something went wrong. Try again.",
    retryable: true,
  };
}
