const NEAR_BOTTOM_PX = 40;

/** True when the view is at (or within a few pixels of) the bottom, so new text should scroll into view. */
export function shouldFollow(
  scrollTop: number,
  clientHeight: number,
  scrollHeight: number,
): boolean {
  return scrollHeight - scrollTop - clientHeight <= NEAR_BOTTOM_PX;
}
