import type { ReactNode } from "react";
/** The whale mark: white body with a navy eye, for use on the navy brand panel. */
export function WhaleMark({ size = 22 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
      focusable="false"
    >
      <path
        d="M3 14c0-5 4-9 9.5-9 4.5 0 8.5 3 8.5 7 0 3-2.5 5-6 5H9l-4 3 .8-3.6C4 15.8 3 15 3 14Z"
        fill="#fff"
      />
      <circle cx="15.5" cy="10.5" r="1.1" fill="#0F2B3C" />
    </svg>
  );
}

interface IconProps {
  size?: number;
}

function Stroke({
  size,
  strokeWidth = 2,
  children,
}: IconProps & { strokeWidth?: number; children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  );
}

/** The whale mark in the current text colour, for the sidebar brand. */
export function WhaleGlyph({ size = 20 }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
      focusable="false"
    >
      <path
        d="M3 14c0-5 4-9 9.5-9 4.5 0 8.5 3 8.5 7 0 3-2.5 5-6 5H9l-4 3 .8-3.6C4 15.8 3 15 3 14Z"
        fill="currentColor"
      />
      <circle cx="15.5" cy="10.5" r="1.1" style={{ fill: "var(--floe)" }} />
    </svg>
  );
}

export const PlusIcon = ({ size = 14 }: IconProps) => (
  <Stroke size={size} strokeWidth={2.2}>
    <path d="M12 5v14M5 12h14" />
  </Stroke>
);

export const SendIcon = ({ size = 15 }: IconProps) => (
  <Stroke size={size} strokeWidth={2.4}>
    <path d="M12 19V5M6 11l6-6 6 6" />
  </Stroke>
);

export const StopIcon = ({ size = 14 }: IconProps) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    fill="currentColor"
    aria-hidden="true"
    focusable="false"
  >
    <rect x="6" y="6" width="12" height="12" rx="2" />
  </svg>
);

export const PencilIcon = ({ size = 13 }: IconProps) => (
  <Stroke size={size}>
    <path d="M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16v4Z" />
  </Stroke>
);

export const TrashIcon = ({ size = 13 }: IconProps) => (
  <Stroke size={size}>
    <path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3" />
  </Stroke>
);

export const GearIcon = ({ size = 14 }: IconProps) => (
  <Stroke size={size}>
    <circle cx="12" cy="12" r="3" />
    <path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M19.1 4.9 17 7M7 17l-2.1 2.1" />
  </Stroke>
);
