import { useLayoutEffect, useRef } from "react";

/**
 * A highlight that slides to whichever option is selected (the child with
 * class "on"), instead of each option fading its own background in and out.
 * Drop it in as the first child of any group of options: tabs, a segmented
 * control, chips, the sidebar. It follows the selection by watching class
 * changes, so the options themselves don't change at all.
 */
export function SlidingPill({ selector = ".on", glideFar = false }: { selector?: string; glideFar?: boolean }) {
  const ref = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    const pill = ref.current;
    const box = pill?.parentElement;
    if (!pill || !box) return;
    box.classList.add("has-pill");
    let first = true;
    let frame = 0;
    let last: { x: number; y: number } | null = null;
    const place = () => {
      const on = box.querySelector<HTMLElement>(`:scope > ${selector}`);
      if (!on) {
        pill.style.opacity = "0";
        last = null;
        return;
      }
      const x = on.offsetLeft;
      const y = on.offsetTop;
      // The first placement jumps straight there; after that it glides to a
      // neighbour. A long jump (the sidebar from the top to Settings) snaps
      // instead: gliding across a gap looks like something going wrong.
      // (A thin bar, like the sidebar's, glides any distance: it's small enough to.)
      const far = !glideFar && !!last && Math.abs(x - last.x) + Math.abs(y - last.y) > 150;
      const jump = first || far;
      if (jump) pill.style.transition = "none";
      pill.style.opacity = "1";
      pill.style.transform = `translate(${x}px, ${y}px)`;
      pill.style.width = `${on.offsetWidth}px`;
      pill.style.height = `${on.offsetHeight}px`;
      pill.style.borderRadius = getComputedStyle(on).borderRadius;
      if (jump) {
        void pill.offsetWidth;
        pill.style.transition = "";
        first = false;
      }
      last = { x, y };
    };
    const soon = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(place);
    };
    place();
    const mo = new MutationObserver(soon);
    mo.observe(box, { subtree: true, attributes: true, attributeFilter: ["class"], childList: true });
    const ro = new ResizeObserver(soon);
    ro.observe(box);
    return () => {
      mo.disconnect();
      ro.disconnect();
      cancelAnimationFrame(frame);
      box.classList.remove("has-pill");
    };
  }, [selector, glideFar]);
  return <span ref={ref} className="slide-pill" aria-hidden="true" />;
}
