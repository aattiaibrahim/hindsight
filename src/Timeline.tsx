import { RefObject, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

/** One day section in the library: its date, and how many clips. */
export type TimelineDay = { key: string; ts: number; count: number };

/** `month` / `year`: the labels it may show; `monthAt`: this day starts a month. */
type Mark = { key: string; ts: number; y: number; month?: string; year?: string; monthAt?: string };

/**
 * Immich-style date scrubber down the right edge of the library: a tick per
 * day, month (and year) labels, a bubble with the date under the pointer,
 * and click or drag to jump. Positions follow the real layout of the day
 * sections, so a day with many clips takes up more of the rail.
 */
export function Timeline({ scroller, days }: { scroller: RefObject<HTMLDivElement | null>; days: TimelineDay[] }) {
  const rail = useRef<HTMLDivElement>(null);
  const viewBox = useRef<HTMLDivElement>(null);
  const [marks, setMarks] = useState<Mark[]>([]);
  // The "you are here" box moves on every scroll: set straight on the element,
  // once per frame, never through React (re-rendering every tick was the stutter).
  const placeView = useCallback(() => {
    const sc = scroller.current;
    const v = viewBox.current;
    const h = rail.current?.clientHeight ?? 0;
    if (!sc || !v) return;
    const total = Math.max(1, sc.scrollHeight);
    v.style.transform = `translateY(${(sc.scrollTop / total) * h}px)`;
    v.style.height = `${Math.max(3, (sc.clientHeight / total) * h)}px`;
  }, [scroller]);
  const [hover, setHover] = useState<{ y: number; label: string; count: number } | null>(null);
  const dragging = useRef(false);

  // Where each day section sits, as a share of the whole scroll height.
  const measure = useCallback(() => {
    const sc = scroller.current;
    const r = rail.current;
    if (!sc || !r) return;
    const total = Math.max(1, sc.scrollHeight);
    const h = r.clientHeight;
    const now = new Date();
    let lastMonth = "";
    let lastYear = "";
    let lastLabelY = -99;
    const out: Mark[] = [];
    for (const d of days) {
      const el = sc.querySelector<HTMLElement>(`[data-day="${CSS.escape(d.key)}"]`);
      if (!el) continue;
      const y = (el.offsetTop / total) * h;
      const date = new Date(d.ts);
      const month = date.toLocaleDateString(undefined, { month: "short" });
      const year = String(date.getFullYear());
      const m: Mark = { key: d.key, ts: d.ts, y };
      if (month !== lastMonth || year !== lastYear) m.monthAt = month;
      // Like Immich: every year gets a label where it starts (the top too);
      // months get one where they change, if there's room.
      if (year !== lastYear) {
        m.year = year;
        if (`${month} ${year}` !== `${lastMonth} ${lastYear}` && y - lastLabelY > 26) m.month = month;
        lastLabelY = y;
      } else if (month !== lastMonth && y - lastLabelY > 16) {
        m.month = month;
        lastLabelY = y;
      }
      void now;
      lastMonth = month;
      lastYear = year;
      out.push(m);
    }
    setMarks(out);
    placeView();
  }, [scroller, days, placeView]);

  useLayoutEffect(measure, [measure]);
  useEffect(() => {
    const sc = scroller.current;
    if (!sc) return;
    let frame = 0;
    const onScroll = () => {
      if (!frame)
        frame = requestAnimationFrame(() => {
          frame = 0;
          placeView();
        });
    };
    sc.addEventListener("scroll", onScroll, { passive: true });
    // Thumbnails loading, the window resizing: re-measure, at most once a frame.
    let measuring = 0;
    const ro = new ResizeObserver(() => {
      if (!measuring)
        measuring = requestAnimationFrame(() => {
          measuring = 0;
          measure();
        });
    });
    ro.observe(sc);
    if (sc.firstElementChild) ro.observe(sc.firstElementChild);
    Array.from(sc.children).forEach((c) => ro.observe(c));
    return () => {
      sc.removeEventListener("scroll", onScroll);
      ro.disconnect();
      cancelAnimationFrame(frame);
      cancelAnimationFrame(measuring);
    };
  }, [scroller, measure, placeView]);

  const dayAt = (y: number) => {
    let best = marks[0];
    for (const m of marks) if (m.y <= y + 1) best = m;
    return best;
  };
  const label = (ts: number) =>
    new Date(ts).toLocaleDateString(undefined, {
      weekday: "short",
      month: "short",
      day: "numeric",
      year: new Date(ts).getFullYear() === new Date().getFullYear() ? undefined : "numeric",
    });
  // A click snaps to the start of the day under the pointer (like Immich); a drag
  // follows the pointer smoothly instead of hopping from day to day.
  const jump = (clientY: number, snap = true) => {
    const sc = scroller.current;
    const r = rail.current;
    if (!sc || !r) return;
    const rect = r.getBoundingClientRect();
    const y = Math.max(0, Math.min(rect.height, clientY - rect.top));
    const d = dayAt(y);
    // Snap to the start of the day under the pointer, like Immich.
    const el = snap && d && sc.querySelector<HTMLElement>(`[data-day="${CSS.escape(d.key)}"]`);
    // Dragging keeps the pointer in the middle of the "you are here" box.
    sc.scrollTop = el ? el.offsetTop - 8 : (y / rect.height) * sc.scrollHeight - sc.clientHeight / 2;
  };
  const move = (e: React.PointerEvent) => {
    const rect = rail.current!.getBoundingClientRect();
    const y = Math.max(0, Math.min(rect.height, e.clientY - rect.top));
    const d = dayAt(y);
    setHover(d ? { y, label: label(d.ts), count: days.find((x) => x.key === d.key)?.count ?? 0 } : null);
    if (dragging.current) jump(e.clientY, false);
  };

  if (days.length < 2) return <div className="timeline-rail empty-rail" ref={rail} />;

  // Labels never move. Years always get theirs (a month just above one gives way);
  // months only where there's room, so nothing piles up at the ends.
  const GAP = 14;
  const shown: { m: Mark; text: string | null }[] = marks.map((m) => ({ m, text: null }));
  let last = -1;
  shown.forEach((x, i) => {
    const want = x.m.year ?? x.m.month;
    if (!want) return;
    if (last >= 0 && x.m.y - shown[last].m.y < GAP) {
      if (!x.m.year || shown[last].m.year) return; // no room
      shown[last].text = null; // a year beats the month above it
    }
    x.text = want;
    last = i;
  });
  return (
    <div
      className={`timeline-rail ${hover ? "active" : ""}`}
      ref={rail}
      onPointerMove={move}
      onPointerLeave={() => !dragging.current && setHover(null)}
      onPointerDown={(e) => {
        dragging.current = true;
        e.currentTarget.setPointerCapture(e.pointerId);
        jump(e.clientY);
      }}
      onPointerUp={() => {
        dragging.current = false;
      }}
      role="scrollbar"
      aria-label="Jump to a date"
      aria-orientation="vertical"
    >
      <div className="tl-view" ref={viewBox} />
      {shown.map(({ m, text }) => (
        <span key={m.key} className={`tl-day ${text ? "month" : ""} ${m.year && text ? "year" : ""}`} style={{ top: m.y }}>
          {text && <em>{m.year && text === m.year ? <b>{text}</b> : text}</em>}
        </span>
      ))}
      {hover && (
        <div className="tl-bubble" style={{ top: hover.y }}>
          {hover.label}
          {hover.count > 0 && <small>{hover.count} clip{hover.count === 1 ? "" : "s"}</small>}
        </div>
      )}
    </div>
  );
}
