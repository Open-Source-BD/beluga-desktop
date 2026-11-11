import type { UiError } from "../../lib/ipc";
import { nextStep } from "./errors";

interface Props {
  error: UiError;
  onSignInAgain: () => void;
  onPickModel: () => void;
  onRetry: () => void;
}

/** An error's message with the single next step that suits its kind. */
export default function ErrorNotice({
  error,
  onSignInAgain,
  onPickModel,
  onRetry,
}: Props) {
  const step = nextStep(error);
  const act = {
    sign_in: onSignInAgain,
    pick_model: onPickModel,
    retry: onRetry,
  };
  return (
    <div className="notice">
      <p role="alert">{error.message}</p>
      {step && (
        <button type="button" className="btn" onClick={act[step.action]}>
          {step.label}
        </button>
      )}
    </div>
  );
}
