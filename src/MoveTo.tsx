import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { GameIcon } from "./GameIcon";
import { tidyGameName } from "./GameNameInput";

const key = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, "");

/**
 * Move the selected clips into a folder: pick one of your folders (with its logo),
 * or type a name to make a new one. Type to filter; ↑ ↓ and Enter pick, Esc closes.
 */
export function MoveToDialog({
  count,
  folders,
  current,
  busy,
  onMove,
  onCancel,
}: {
  count: number;
  /** Every folder in the library, one per spelling */
  folders: string[];
  /** The folder all the selected clips are already in (left out of the list), if they share one */
  current: string | null;
  busy: boolean;
  onMove: (folder: string) => void;
  onCancel: () => void;
}) {
  const [text, setText] = useState("");
  const [hi, setHi] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const q = key(text);
  const typed = tidyGameName(text);
  const options = useMemo(() => {
    const own = folders
      .filter((f) => f && (!current || key(f) !== key(current)))
      .filter((f) => !q || key(f).includes(q))
      .sort((a, b) => Number(!key(a).startsWith(q)) - Number(!key(b).startsWith(q)) || a.localeCompare(b));
    const exists = folders.some((f) => key(f) === q);
    return [...own.map((name) => ({ name, isNew: false })), ...(typed && !exists ? [{ name: typed, isNew: true }] : [])];
  }, [folders, current, q, typed]);
  useEffect(() => setHi(0), [text]);
  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-i="${hi}"]`)?.scrollIntoView({ block: "nearest" });
  }, [hi]);
  const pick = (i: number) => {
    const o = options[i];
    if (o && !busy) onMove(o.name);
  };

  return createPortal(
    <div className="modal-backdrop" onClick={busy ? undefined : onCancel}>
      <div className="modal move-to" role="dialog" aria-modal="true" aria-labelledby="move-to-title" onClick={(e) => e.stopPropagation()}>
        <h2 id="move-to-title">
          Move {count} clip{count === 1 ? "" : "s"} to…
        </h2>
        <input
          className="text-input"
          autoFocus
          placeholder="Find a folder, or type a new name"
          value={text}
          maxLength={60}
          disabled={busy}
          aria-label="Folder"
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              setHi((i) => Math.max(0, Math.min(options.length - 1, i + (e.key === "ArrowDown" ? 1 : -1))));
            } else if (e.key === "Enter") {
              e.preventDefault();
              pick(hi);
            } else if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              if (!busy) onCancel();
            }
          }}
        />
        <div className="move-to-list" ref={listRef} role="listbox">
          {options.map((o, i) => (
            <button
              key={(o.isNew ? "new:" : "") + o.name}
              data-i={i}
              role="option"
              aria-selected={i === hi}
              className={`move-to-item ${i === hi ? "hi" : ""}`}
              disabled={busy}
              onPointerEnter={() => setHi(i)}
              onClick={() => pick(i)}
            >
              {o.isNew ? <span className="move-to-plus">+</span> : <GameIcon name={o.name} size={20} />}
              <span className="move-to-name">{o.isNew ? `New folder “${o.name}”` : o.name}</span>
            </button>
          ))}
          {!options.length && <p className="muted small">No folder by that name.</p>}
        </div>
        <p className="muted small" style={{ margin: 0 }}>
          Moves them on this PC and the NAS together. You can undo it afterwards.
        </p>
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button className="btn" onClick={onCancel} disabled={busy}>
            {busy ? "Moving…" : "Cancel"}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
