import { useEffect, useMemo, useRef, useState } from "react";
import { ClipFile } from "./api";
import { GameIcon } from "./GameIcon";
import { SlidingPill } from "./SlidingPill";

const TOP = 8;

/** Same game, any spelling: "VALORANT" and "Valorant". */
export const gameKey = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, "");

type Game = { name: string; key: string; count: number; latest: number };

/**
 * Choosing a game without a wall of chips: "All", the games you've clipped
 * most recently, and "More games" for a searchable list of every game with
 * its logo and clip count. The chosen game always shows as a chip.
 */
export function GamePicker({
  clips,
  folder,
  setFolder,
  onNewGame,
}: {
  clips: ClipFile[];
  folder: string;
  setFolder: (f: string) => void;
  /** "+ New game": make a game's folder and move clips into it */
  onNewGame?: () => void;
}) {
  const games = useMemo(() => {
    const by = new Map<string, Game>();
    for (const c of clips) {
      const key = gameKey(c.folder);
      const g = by.get(key) ?? { name: c.folder, key, count: 0, latest: 0 };
      g.count++;
      g.latest = Math.max(g.latest, c.modified);
      // Prefer this PC's spelling for the label.
      if (c.location === "local") g.name = c.folder;
      by.set(key, g);
    }
    return [...by.values()];
  }, [clips]);
  const recent = useMemo(() => [...games].sort((a, b) => b.latest - a.latest).slice(0, TOP), [games]);
  const chosen = folder !== "All" && !recent.some((g) => g.key === gameKey(folder)) ? games.find((g) => g.key === gameKey(folder)) : undefined;
  const chips = chosen ? [...recent, chosen] : recent;
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const box = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (!open) return;
    input.current?.focus();
    const onDown = (e: PointerEvent) => !box.current?.contains(e.target as Node) && setOpen(false);
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("pointerdown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);
  const list = games.filter((g) => !q || g.name.toLowerCase().includes(q.toLowerCase())).sort((a, b) => a.name.localeCompare(b.name));
  const pick = (name: string) => {
    setFolder(name);
    setOpen(false);
    setQ("");
  };
  if (games.length < 2 && !onNewGame) return null;
  return (
    <div className="chips game-picker">
      <SlidingPill />
      <button className={`chip ${folder === "All" ? "on" : ""}`} onClick={() => setFolder("All")}>
        All
      </button>
      {chips.map((g) => (
        <button key={g.key} className={`chip game-chip ${gameKey(folder) === g.key ? "on" : ""}`} onClick={() => setFolder(g.name)} title={`${g.count} clip${g.count === 1 ? "" : "s"}`}>
          <GameIcon name={g.name} size={16} />
          {g.name}
        </button>
      ))}
      {games.length > chips.length && (
        <div className="more-games" ref={box}>
          <button className={`chip more-chip ${open ? "open" : ""}`} onClick={() => setOpen((o) => !o)} aria-expanded={open}>
            More games ({games.length - chips.length}) ▾
          </button>
          {open && (
            <div className="more-menu" role="dialog" aria-label="All games">
              <input ref={input} className="text-input" placeholder={`Search ${games.length} games…`} value={q} onChange={(e) => setQ(e.target.value)} />
              <div className="more-list">
                {list.map((g) => (
                  <button key={g.key} className={`more-item ${gameKey(folder) === g.key ? "on" : ""}`} onClick={() => pick(g.name)}>
                    <GameIcon name={g.name} size={18} />
                    <span>{g.name}</span>
                    <small className="muted">{g.count}</small>
                  </button>
                ))}
                {!list.length && <p className="muted" style={{ margin: 8 }}>No game matches “{q}”.</p>}
              </div>
            </div>
          )}
        </div>
      )}
      {onNewGame && (
        <button className="chip new-game-chip" onClick={onNewGame} title="Make a folder for a game and move clips into it">
          + New game
        </button>
      )}
    </div>
  );
}
