const KEY = "sendWithModifier";

/** Whether Cmd/Ctrl+Enter (not Enter) sends. */
export function readSendWithModifier(): boolean {
  try {
    return localStorage.getItem(KEY) === "true";
  } catch {
    return false;
  }
}

export function writeSendWithModifier(on: boolean): void {
  try {
    localStorage.setItem(KEY, String(on));
  } catch {
    // The choice then lasts only until the app closes.
  }
}
