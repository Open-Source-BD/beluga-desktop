export type Appearance = "system" | "light" | "dark";

const KEY = "appearance";

function isAppearance(v: unknown): v is Appearance {
  return v === "system" || v === "light" || v === "dark";
}

export function getAppearance(): Appearance {
  try {
    const v = localStorage.getItem(KEY);
    return isAppearance(v) ? v : "system";
  } catch {
    return "system";
  }
}

export function applyAppearance(a: Appearance): void {
  const root = document.documentElement;
  if (a === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", a);
}

export function setAppearance(a: Appearance): void {
  try {
    localStorage.setItem(KEY, a);
  } catch {
    // storage unavailable: still apply for this session
  }
  applyAppearance(a);
}
