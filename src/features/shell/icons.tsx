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
