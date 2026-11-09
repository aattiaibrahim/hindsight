import { Children, isValidElement, ReactElement, ReactNode, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

type Opt = { value: string; label: ReactNode; text: string };

/**
 * The app's dropdown, in place of Windows' own list: a dark floating menu where
 * the current choice is marked by the same accent edge bar as the sidebar.
 * Takes the same props as a <select> with <option> children, and calls
 * `onChange({ target: { value } })`, so it swaps in without touching handlers.
 * Keyboard: Enter, Space or the arrows open it; arrows move, Enter picks, Esc
 * closes, and typing jumps to an option starting with that letter.
 */
export function Select({
  value,
  onChange,
  children,
  className = "select",
  disabled,
  title,
  style,
  "aria-label": ariaLabel,
}: {
  value: string | number;
  onChange: (e: { target: { value: string } }) => void;
  children: ReactNode;
  className?: string;
  disabled?: boolean;
  title?: string;
  style?: React.CSSProperties;
  "aria-label"?: string;
}) {
  const opts: Opt[] = [];
  Children.forEach(children, (c) => {
    if (!isValidElement(c) || c.type !== "option") return;
    const p = (c as ReactElement<{ value?: string | number; children?: ReactNode }>).props;
    const label = p.children;
    const text = typeof label === "string" || typeof label === "number" ? String(label) : Children.toArray(label).filter((x) => typeof x === "string" || typeof x === "number").join("");
    opts.push({ value: String(p.value ?? text), label, text });
  });
  const current = String(value);
  const at = Math.max(0, opts.findIndex((o) => o.value === current));
  const [open, setOpen] = useState(false);
  const [hi, setHi] = useState(at);
  const [pos, setPos] = useState<{ left: number; top: number; width: number; up: boolean } | null>(null);
  const btn = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const id = useId();

  const place = () => {
    const r = btn.current?.getBoundingClientRect();
    if (!r) return;
    const h = Math.min(320, opts.length * 34 + 10);
    const up = r.bottom + 6 + h > window.innerHeight - 8 && r.top - 6 - h > 8;
    setPos({ left: Math.min(r.left, window.innerWidth - Math.max(r.width, 180) - 8), top: up ? r.top - 6 : r.bottom + 6, width: r.width, up });
  };
  const show = () => {
    if (disabled) return;
    setHi(at);
    place();
    setOpen(true);
  };
  const pick = (i: number) => {
    const o = opts[i];
    setOpen(false);
    btn.current?.focus();
    if (o && o.value !== current) onChange({ target: { value: o.value } });
  };

  // Close on a click elsewhere, a scroll of the page behind, or the window resizing.
  useEffect(() => {
    if (!open) return;
    const away = (e: Event) => {
      const t = e.target as Node;
      if (!menu.current?.contains(t) && !btn.current?.contains(t)) setOpen(false);
    };
    const scroll = (e: Event) => !menu.current?.contains(e.target as Node) && setOpen(false);
    const close = () => setOpen(false);
    document.addEventListener("pointerdown", away, true);
    window.addEventListener("scroll", scroll, true);
    window.addEventListener("resize", close);
    window.addEventListener("blur", close);
    return () => {
      document.removeEventListener("pointerdown", away, true);
      window.removeEventListener("scroll", scroll, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("blur", close);
    };
  }, [open]);
  // Keep the highlighted option in view.
  useLayoutEffect(() => {
    if (open) menu.current?.querySelector<HTMLElement>(`[data-i="${hi}"]`)?.scrollIntoView({ block: "nearest" });
  }, [open, hi]);

  const key = (e: React.KeyboardEvent) => {
    if (!open) {
      if (["Enter", " ", "ArrowDown", "ArrowUp"].includes(e.key)) {
        e.preventDefault();
        show();
      }
      return;
    }
    if (e.key === "Escape" || e.key === "Tab") {
      if (e.key === "Escape") e.preventDefault();
      setOpen(false);
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setHi((i) => Math.min(opts.length - 1, i + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setHi((i) => Math.max(0, i - 1));
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      setHi(e.key === "Home" ? 0 : opts.length - 1);
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      pick(hi);
    } else if (e.key.length === 1) {
      const k = e.key.toLowerCase();
      const n = opts.findIndex((o, i) => i > hi && o.text.toLowerCase().startsWith(k));
      const first = opts.findIndex((o) => o.text.toLowerCase().startsWith(k));
      if (n >= 0 || first >= 0) setHi(n >= 0 ? n : first);
    }
  };

  return (
    <>
      <button
        ref={btn}
        type="button"
        className={`${className} dd-trigger ${open ? "open" : ""}`}
        style={style}
        title={title}
        disabled={disabled}
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        onClick={(e) => {
          e.preventDefault();
          if (open) setOpen(false);
          else show();
        }}
        onKeyDown={key}
      >
        <span className="dd-value">{opts[at]?.label ?? ""}</span>
        <svg className="dd-chevron" viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
          <path d="M4 6l4 4 4-4" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>
      {open &&
        pos &&
        createPortal(
          <div
            ref={menu}
            id={id}
            className={`dd-menu ${pos.up ? "up" : ""}`}
            role="listbox"
            aria-label={ariaLabel}
            style={{ left: pos.left, top: pos.top, minWidth: Math.max(pos.width, 160) }}
            onKeyDown={key}
          >
            {opts.map((o, i) => (
              <div
                key={o.value}
                data-i={i}
                role="option"
                aria-selected={o.value === current}
                className={`dd-item ${o.value === current ? "on" : ""} ${i === hi ? "hi" : ""}`}
                onPointerEnter={() => setHi(i)}
                onPointerDown={(e) => e.preventDefault()}
                onClick={() => pick(i)}
              >
                {o.label}
              </div>
            ))}
          </div>,
          document.body,
        )}
    </>
  );
}
