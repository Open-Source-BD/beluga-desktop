import { useEffect } from "react";

/**
 * A file dropped on a web view navigates it to that file, replacing the app.
 * Prevent that for every drag on every screen; the conversation handles its
 * own drops and decides itself whether to attach.
 */
export function useDropGuard(): void {
  useEffect(() => {
    const stop = (e: Event) => e.preventDefault();
    window.addEventListener("dragover", stop);
    window.addEventListener("drop", stop);
    return () => {
      window.removeEventListener("dragover", stop);
      window.removeEventListener("drop", stop);
    };
  }, []);
}
