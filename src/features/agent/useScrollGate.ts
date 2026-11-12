import { useCallback, useEffect, useRef, useState } from "react";
import { atEnd, needsScroll } from "./measure";

/**
 * A scrolling block whose content might not fit. `gated` is true while it overflows and has
 * not been scrolled to its end at least once: the card keeps its approve buttons off until then.
 */
export function useScrollGate(content: unknown) {
  const el = useRef<HTMLElement | null>(null);
  const [overflow, setOverflow] = useState(false);
  const [reached, setReached] = useState(false);
  const measure = useCallback(() => {
    if (el.current) setOverflow(needsScroll(el.current));
  }, []);
  const ref = useCallback(
    (node: HTMLElement | null) => {
      el.current = node;
      if (node) measure();
    },
    [measure],
  );
  useEffect(() => {
    measure();
    const node = el.current;
    if (!node || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(node);
    return () => ro.disconnect();
  }, [content, measure]);
  const onScroll = useCallback(() => {
    if (el.current && atEnd(el.current)) setReached(true);
  }, []);
  /** Scrolls to the end for a reader whose cursor does not scroll the block. */
  const showEnd = useCallback(() => {
    const node = el.current;
    if (!node) return;
    node.scrollTop = node.scrollHeight;
    if (atEnd(node)) setReached(true);
  }, []);
  return {
    ref,
    onScroll,
    /** Props for the block: any way of arriving at its end counts. */
    handlers: { onScroll, onFocus: onScroll, onKeyUp: onScroll },
    showEnd,
    overflow,
    gated: overflow && !reached,
  };
}
