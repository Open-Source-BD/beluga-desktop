import type { UiError } from "../../lib/ipc";
import { nextStep } from "./errors";

interface Props {
  error: UiError;
  onSignInAgain: () => void;
  onPickModel: () => void;
  onRetry: () => void;
  /** A task that ran tools cannot be retried as a whole: its Retry becomes Continue. */
  onContinue?: () => void;
}

/** An error's message with the single next step that suits its kind. */
export default function ErrorNotice({
  error,
  onSignInAgain,
  onPickModel,
  onRetry,
  onContinue,
}: Props) {
  const step = nextStep(error);
  const act = {
    sign_in: onSignInAgain,
    pick_model: onPickModel,
    retry: onContinue ?? onRetry,
  };
  return (
    <div className="notice">
      <p role="alert">{error.message}</p>
      {step && (
        <button type="button" className="btn" onClick={act[step.action]}>
          {step.action === "retry" && onContinue ? "Continue" : step.label}
        </button>
      )}
    </div>
  );
}
