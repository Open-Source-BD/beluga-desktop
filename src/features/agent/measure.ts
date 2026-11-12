/** Layout measurements, kept apart so tests (which have no layout) can stand in for them. */

/** The element's content is taller than its box. */
export const needsScroll = (el: HTMLElement): boolean => el.scrollHeight > el.clientHeight;

/**
 * Pixels short of the end that still count as the end: at 125% or 150% display scaling the
 * sizes are fractional and a box scrolled all the way can stay a few pixels short.
 */
export const END_SLACK = 4;

/** The element is scrolled to its end, within `END_SLACK`. */
export const atEnd = (el: HTMLElement): boolean => el.scrollTop + el.clientHeight >= el.scrollHeight - END_SLACK;
