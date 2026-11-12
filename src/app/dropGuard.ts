import { useEffect } from "react";

/** True when the drag carries files (not just text or a link). */
export function dragHasFiles(e: { dataTransfer: DataTransfer | null }): boolean {
  return Array.from(e.dataTransfer?.types ?? []).includes("Files");
}

const TEXT_INPUTS = new Set(["text", "search", "url", "tel", "email", "password", "number", ""]);

function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  if (target instanceof HTMLTextAreaElement) return true;
  return target instanceof HTMLInputElement && TEXT_INPUTS.has(target.type);
}

/**
 * A file dropped on a web view navigates it to that file, replacing the app.
 * Prevent that for every drag on every screen, except a text drag onto an
 * editable field, which the browser must handle to move or insert text. A
 * drag with files is always prevented; the conversation attaches those itself.
 */
export function useDropGuard(): void {
  useEffect(() => {
    const guard = (e: Event) => {
      const drag = e as DragEvent;
      if (isEditable(e.target) && !dragHasFiles(drag)) return;
      e.preventDefault();
    };
    window.addEventListener("dragover", guard);
    window.addEventListener("drop", guard);
    return () => {
      window.removeEventListener("dragover", guard);
      window.removeEventListener("drop", guard);
    };
  }, []);
}
