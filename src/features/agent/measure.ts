/** Layout measurements, kept apart so tests (which have no layout) can stand in for them. */

/** The element's content is taller than its box. */
export const needsScroll = (el: HTMLElement): boolean => el.scrollHeight > el.clientHeight;

/** The element is scrolled to its end (a pixel of slack for fractional sizes). */
export const atEnd = (el: HTMLElement): boolean => el.scrollHeight - el.scrollTop - el.clientHeight <= 1;
