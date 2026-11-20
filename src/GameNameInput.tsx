import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { api } from "./api";
import { GameIcon } from "./GameIcon";

const key = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, "");
/** Safe as a folder name: no characters Windows refuses, single spaces. */
export const tidyGameName = (s: string) => s.replace(/[<>:"/\\|?*]/g, " ").replace(/\s+/g, " ").trim();

/**
 * Typing a game's name, with suggestions as you type: your own game folders
 * first, then games on Steam (when Online game logos is on). Click one, or use
 * ↑ ↓ and Enter; Enter with nothing picked uses what you typed, Esc cancels.
 */
export function GameNameInput({ games, onPick, onCancel }: { games: string[]; onPick: (name: string) => void; onCancel: () => void }) {
  const [text, setText] = useState("");
  const [steam, setSteam] = useState<string[]>([]);
  const [hi, setHi] = useState(-1);
  const [pos, setPos] = useState<{ left: number; top: number; width: number } | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const q = key(text);
  // Your folders: names starting with what you typed first, then names containing it.
  const mine = q
    ? games
        .filter((g) => key(g).includes(q))
        .sort((a, b) => Number(!key(a).startsWith(q)) - Number(!key(b).startsWith(q)) || a.localeCompare(b))
        .slice(0, 5)
    : [];
  const fromSteam = steam.filter((s) => !games.some((g) => key(g) === key(s))).slice(0, 5);
  const list = [...mine.map((name) => ({ name, mine: true })), ...fromSteam.map((name) => ({ name, mine: false }))];
  // Steam, a moment after you stop typing (not on every key).
  useEffect(() => {
    setSteam([]);
    if (text.trim().length < 2) return;
    let alive = true;
    const t = setTimeout(() => {
      api
        .searchGames(text)
        .then((r) => alive && setSteam(Array.isArray(r) ? r : []))
        .catch(() => {});
    }, 300);
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [text]);
  useEffect(() => setHi(-1), [text]);
  useLayoutEffect(() => {
    const r = input.current?.getBoundingClientRect();
    if (r) setPos({ left: r.left, top: r.bottom + 4, width: Math.max(r.width, 240) });
  }, [list.length]);
  const use = (name: string) => {
    const n = tidyGameName(name);
    if (n) onPick(games.find((g) => key(g) === key(n)) ?? n);
  };
  return (
    <>
      <input
        ref={input}
        className="text-input sort-new"
        autoFocus
        placeholder="Type the game's name"
        value={text}
        maxLength={60}
        aria-label="Game's name"
        aria-autocomplete="list"
        onChange={(e) => setText(e.target.value)}
        onClick={(e) => e.preventDefault()}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            if (list.length) setHi((i) => (e.key === "ArrowDown" ? Math.min(list.length - 1, i + 1) : Math.max(-1, i - 1)));
          } else if (e.key === "Enter") {
            e.preventDefault();
            use(hi >= 0 && list[hi] ? list[hi].name : text);
          } else if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            onCancel();
          }
        }}
      />
      <button
        type="button"
        className="btn small primary"
        disabled={!tidyGameName(text)}
        onClick={(e) => {
          e.preventDefault();
          use(text);
        }}
      >
        Use
      </button>
      {list.length > 0 &&
        pos &&
        createPortal(
          <div className="dd-menu name-menu" role="listbox" style={{ left: pos.left, top: pos.top, minWidth: pos.width }}>
            {list.map((o, i) => (
              <div
                key={(o.mine ? "m:" : "s:") + o.name}
                role="option"
                aria-selected={i === hi}
                className={`dd-item name-item ${i === hi ? "hi" : ""}`}
                onPointerEnter={() => setHi(i)}
                onPointerDown={(e) => e.preventDefault()}
                onClick={() => use(o.name)}
              >
                {o.mine && <GameIcon name={o.name} size={16} />}
                <span>{o.name}</span>
                <small className="muted">{o.mine ? "your folder" : "Steam"}</small>
              </div>
            ))}
          </div>,
          document.body,
        )}
    </>
  );
}
