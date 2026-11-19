import { Select } from "./Select";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { api, ClipFile, DetectStatus, errText, GameSuggestion } from "./api";
import { GameIcon } from "./GameIcon";
import { useThumb } from "./hooks";
import { ProgressBar, useCountProgress } from "./Progress";

/** Suggestions for unsorted clips, kept fresh as the background job looks at more. */
export function useGameSuggestions(on: boolean) {
  const [list, setList] = useState<GameSuggestion[]>([]);
  const load = useCallback(() => {
    if (on) api.gameSuggestions().then(setList).catch(() => {});
    else setList([]);
  }, [on]);
  useEffect(() => {
    load();
    const a = listen("detect-changed", load);
    const b = listen("clips-changed", load);
    return () => {
      a.then((f) => f());
      b.then((f) => f());
    };
  }, [load]);
  return [list, load] as const;
}

export function useDetectStatus() {
  const [s, setS] = useState<DetectStatus | null>(null);
  useEffect(() => {
    const load = () => api.detectStatus().then(setS).catch(() => {});
    load();
    const t = setInterval(load, 5000);
    const un = listen("detect-changed", load);
    return () => {
      clearInterval(t);
      un.then((f) => f());
    };
  }, []);
  return s;
}

const NEW = "\u0000new";

function Row({ s, on, game, games, onToggle, onGame }: { s: GameSuggestion; on: boolean; game: string; games: string[]; onToggle: () => void; onGame: (g: string) => void }) {
  const thumb = useThumb(s.path);
  const [err, setErr] = useState<string | null>(null);
  const [naming, setNaming] = useState(false);
  const [draft, setDraft] = useState("");
  // A game with no folder yet: its folder is made when the clip moves.
  const addGame = () => {
    const name = draft.replace(/[<>:"/\\|?*]/g, " ").replace(/\s+/g, " ").trim();
    if (!name) return;
    // Same game, other spelling: use the folder that's there.
    const same = games.find((g) => g.toLowerCase().replace(/[^a-z0-9]/g, "") === name.toLowerCase().replace(/[^a-z0-9]/g, ""));
    onGame(same ?? name);
    setNaming(false);
  };
  return (
    <label className={`sort-row ${on ? "" : "off"}`}>
      <input type="checkbox" checked={on} onChange={onToggle} aria-label={`Move ${s.name}`} />
      <span className={`sort-thumb ${thumb ? "" : "loading"}`}>{thumb && <img src={thumb} alt="" />}</span>
      <span className="sort-main">
        <strong title={s.path}>{s.name}</strong>
        <small className="muted">
          in {s.folder} · {Math.round(s.confidence * 100)}% sure
        </small>
        {err && <small className="err-text">{err}</small>}
      </span>
      {/* Have a look at the file itself (it's inside a label: don't tick the box too). */}
      <button
        type="button"
        className="btn small"
        title={`Show ${s.path} in File Explorer`}
        onClick={(e) => {
          e.preventDefault();
          setErr(null);
          api.reveal(s.path).catch((x) => setErr(errText(x)));
        }}
      >
        Show in folder
      </button>
      <span className="sort-game">
        <GameIcon name={naming ? draft : game} size={18} />
        {naming ? (
          <>
            <input
              className="text-input sort-new"
              autoFocus
              placeholder="Game's name"
              value={draft}
              maxLength={60}
              aria-label="New game's name"
              onChange={(e) => setDraft(e.target.value)}
              onClick={(e) => e.preventDefault()}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  addGame();
                } else if (e.key === "Escape") {
                  e.preventDefault();
                  e.stopPropagation();
                  setNaming(false);
                }
              }}
            />
            <button
              type="button"
              className="btn small primary"
              disabled={!draft.trim()}
              onClick={(e) => {
                e.preventDefault();
                addGame();
              }}
            >
              Use
            </button>
          </>
        ) : (
          <Select
            className="select"
            value={game}
            onChange={(e) => {
              if (e.target.value === NEW) {
                setDraft("");
                setNaming(true);
              } else onGame(e.target.value);
            }}
            aria-label="Game"
          >
            {games.map((g) => (
              <option key={g} value={g}>
                {g}
              </option>
            ))}
            <option value={NEW}>+ New game…</option>
          </Select>
        )}
      </span>
    </label>
  );
}

/**
 * Review what the picture check found: each clip with the game it looks like.
 * Untick or change any of them; nothing moves until you press Move, and the
 * move can be undone like any other.
 */
export function GameSortDialog({
  list,
  clips,
  onClose,
  onDone,
}: {
  list: GameSuggestion[];
  clips: ClipFile[];
  onClose: () => void;
  onDone: (moved: number, failed: number) => void;
}) {
  const [off, setOff] = useState<Set<string>>(new Set());
  const [games, setGames] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const known = [...new Set(clips.map((c) => c.folder).filter((f) => !["Desktop", "OBS", "Other", "Imports", "Imported"].includes(f)))];
  // Games named with "New game…": offered in every row's list.
  const added = Object.values(games);
  const options = (game: string) => [...new Set([game, ...added, ...known])].sort((a, b) => a.localeCompare(b, undefined, { sensitivity: "base" }));
  const chosen = list.filter((s) => !off.has(s.path));
  const moved = useCountProgress("move-progress", busy);
  const move = async () => {
    setBusy(true);
    setErr(null);
    try {
      const out = await api.applyGameSort(chosen.map((s) => [s.path, games[s.path] ?? s.game]));
      const turnedDown = list.filter((s) => off.has(s.path)).map((s) => s.path);
      if (turnedDown.length) await api.dismissGameSuggestions(turnedDown);
      onDone(out.moved, out.failed.length);
    } catch (e) {
      setErr(errText(e));
      setBusy(false);
    }
  };
  return createPortal(
    <div className="modal-backdrop" onClick={() => !busy && onClose()}>
      <div className="modal sort-modal" role="dialog" aria-modal="true" aria-label="Sort clips by game" onClick={(e) => e.stopPropagation()}>
        <h2>Found the game for {list.length} clip{list.length === 1 ? "" : "s"}</h2>
        <p className="muted" style={{ margin: 0 }}>
          From what's on screen, compared with your sorted clips. Untick any that look wrong or change the game; ticked clips move
          into that game's folder, on this PC and the NAS. Undo in Settings → Library puts them back. Unticked ones won't be
          suggested again.
        </p>
        <div className="sort-list">
          {list.map((s) => (
            <Row
              key={s.path}
              s={s}
              on={!off.has(s.path)}
              game={games[s.path] ?? s.game}
              games={options(s.game)}
              onToggle={() =>
                setOff((o) => {
                  const n = new Set(o);
                  if (n.has(s.path)) n.delete(s.path);
                  else n.add(s.path);
                  return n;
                })
              }
              onGame={(g) => setGames((m) => ({ ...m, [s.path]: g }))}
            />
          ))}
        </div>
        {err && <small className="err-text">{err}</small>}
        {busy && <ProgressBar pct={moved && moved[1] ? Math.round((moved[0] / moved[1]) * 100) : null} />}
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button className="btn" disabled={busy} onClick={onClose}>
            Later
          </button>
          <button className="btn primary" disabled={busy || chosen.length === 0} onClick={move}>
            {busy ? (moved ? `Moving ${moved[0]} of ${moved[1]}…` : "Moving…") : `Move ${chosen.length} clip${chosen.length === 1 ? "" : "s"}`}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
