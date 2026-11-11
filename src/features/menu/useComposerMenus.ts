import type { KeyboardEvent, ReactNode } from "react";

interface Menu {
  menu: ReactNode;
  fieldAria?: object;
  onKeyIntercept: (e: KeyboardEvent<HTMLTextAreaElement>) => boolean;
  onTextChange: (text: string, caret: number) => void;
}

/**
 * Combines the box's menus. They never apply to the same caret position (`/`
 * needs the first word of the box, `@` a word of its own), so the open one
 * supplies the node, the field attributes and the keys.
 */
export function useComposerMenus(...menus: Menu[]) {
  const open = menus.find((m) => m.menu);
  return {
    menu: open?.menu ?? null,
    fieldAria: open?.fieldAria,
    onKeyIntercept: (e: KeyboardEvent<HTMLTextAreaElement>) =>
      menus.some((m) => m.onKeyIntercept(e)),
    onTextChange: (text: string, caret: number) => menus.forEach((m) => m.onTextChange(text, caret)),
  };
}
