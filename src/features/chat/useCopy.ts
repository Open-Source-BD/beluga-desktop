import { useCallback, useEffect, useRef, useState } from "react";

export type CopyState = "idle" | "copied" | "failed";

const RESET_MS = 2000;

/** Copies text to the clipboard; `state` reads "copied" or "failed" for two seconds, then "idle". */
export function useCopy(write?: (text: string) => Promise<void> | void) {
  const [state, setState] = useState<CopyState>("idle");
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => void (timer.current && clearTimeout(timer.current)), []);

  const copy = useCallback(
    async (text: string) => {
      let next: CopyState = "copied";
      try {
        await (write ? write(text) : navigator.clipboard.writeText(text));
      } catch {
        next = "failed";
      }
      setState(next);
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => setState("idle"), RESET_MS);
    },
    [write],
  );
  return { state, copy };
}

/** The button text for a copy action in each state. */
export function copyLabel(state: CopyState, idle: string): string {
  return state === "copied" ? "Copied" : state === "failed" ? "Could not copy" : idle;
}
