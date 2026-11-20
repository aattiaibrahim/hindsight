import { Select } from "./Select";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { api, ClipFile, DetectStatus, errText, GameSuggestion } from "./api";
import { GameIcon } from "./GameIcon";
import { useSettings } from "./hooks";
import { ClipPlayer, HoverPreview } from "./ClipPreview";
import { GameNameInput } from "./GameNameInput";
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
/** Where "Not a game" puts a clip: the folder for everything that isn't a game. */
const NOT_A_GAME = "Desktop";
const key = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, "");
/** Folders that never name a game: the unsorted ones (as the game finder counts them)… */
const UNSORTED = ["desktop", "obs", "other", "imports", "imported", "recordings", "screenrecording", "captures", "xboxgamebar", "ascent", "clips", "videos", "temp", "untitled"];
/** …and your own work ("Edits", "YT Edit", "funny not cool"). */
const WORKSHOP = /(^|[^a-z0-9])(edit|render|export|youtube|montage|funny|meme|highlight)/i;
const isWorkshop = (f: string) => WORKSHOP.test(f);

function Row({
  s,
  on,
  game,
  games,
  onToggle,
  onGame,
}: {
  s: GameSuggestion;
  on: boolean;
  game: string;
  games: string[];
  onToggle: () => void;
  onGame: (g: string) => void;
}) {
  const [err, setErr] = useState<string | null>(null);
  const [naming, setNaming] = useState(false);
  const [playing, setPlaying] = useState(false);
  const notGame = key(game) === key(NOT_A_GAME);
  // Buttons inside the row's label: don't tick the box as well.
  const button = (run: () => void) => (e: React.MouseEvent) => {
    e.preventDefault();
    run();
  };
  return (
    // Dimmed: a sure one you unticked. A close call waiting for a pick stays readable.
    <label className={`sort-row ${on || !s.sure ? "" : "off"}`}>
      <input type="checkbox" checked={on} onChange={onToggle} aria-label={`Move ${s.name}`} />
      <HoverPreview path={s.path} className="sort-thumb" onOpen={() => setPlaying(true)} />
      {playing && <ClipPlayer path={s.path} title={s.name} subtitle={`in ${s.folder}`} onClose={() => setPlaying(false)} />}
      <span className="sort-main">
        <strong title={s.path}>{s.name}</strong>
        <small className="muted">
          in {s.folder}
          {s.sure && ` · ${Math.round(s.confidence * 100)}% sure`}
        </small>
        {/* A close call: the likeliest games, one click each. */}
        {!s.sure && s.choices.length > 0 && (
          <span className="sort-choices">
            {s.choices.map(([g, share]) => (
              <button
                key={g}
                type="button"
                className={`chip small ${on && game === g ? "on" : ""}`}
                onClick={button(() => onGame(g))}
                title={`Looks most like your ${g} clips`}
              >
                <span>{g}</span> <span className="muted">{Math.round(share * 100)}%</span>
              </button>
            ))}
          </span>
        )}
        {err && <small className="err-text">{err}</small>}
      </span>
      <span className="sort-actions">
        <button
          type="button"
          className="btn small"
          title={`Show ${s.path} in File Explorer`}
          onClick={button(() => {
            setErr(null);
            api.reveal(s.path).catch((x) => setErr(errText(x)));
          })}
        >
          Show in folder
        </button>
        <button
          type="button"
          className={`btn small ${on && notGame ? "primary" : ""}`}
          title={
            key(s.folder) === key(NOT_A_GAME)
              ? "It's not a game: leave it in Desktop and stop suggesting it"
              : "It's not a game: move it to Desktop, so footage like it isn't taken for a game"
          }
          onClick={button(() => onGame(NOT_A_GAME))}
        >
          Not a game
        </button>
      </span>
      <span className="sort-game">
        {naming ? (
          // A game with no folder yet: its folder is made when the clip moves.
          <GameNameInput
            games={games}
            onPick={(name) => {
              onGame(name);
              setNaming(false);
            }}
            onCancel={() => setNaming(false)}
          />
        ) : (
          <Select
            className="select"
            value={game}
            onChange={(e) => {
              if (e.target.value === NEW) setNaming(true);
              else onGame(e.target.value);
            }}
            // Each game's logo, in the list and on the button; none for "Not a game" and "+ New game…".
            icon={(v) => (v === NEW || key(v) === key(NOT_A_GAME) ? <span style={{ width: 16 }} /> : <GameIcon name={v} size={16} />)}
            aria-label="Game"
          >
            {games.map((g) => (
              <option key={g} value={g}>
                {g}
              </option>
            ))}
            <option value={NOT_A_GAME}>Not a game (Desktop)</option>
            <option value={NEW}>+ New game…</option>
          </Select>
        )}
      </span>
    </label>
  );
}

/**
 * Folders that aren't a game (your own, like "aim" or "BenQ"): ticked ones are
 * never learned from and never offered as a clip's game. Folders with no game
 * logo come first, since they're the likely ones. `folders`: the library's
 * folders, if the caller has them (else they're read here).
 */
export function NotGameFolders({ folders, onClose }: { folders?: string[]; onClose: () => void }) {
  const [settings, update] = useSettings();
  const [all, setAll] = useState<string[] | null>(folders ?? null);
  const [logo, setLogo] = useState<Record<string, boolean>>({});
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    if (!all)
      api
        .listClips()
        .then((c) => setAll(c.map((x) => x.folder)))
        .catch((e) => setErr(errText(e)));
  }, [all]);
  // One entry per game, however it's spelled; never the unsorted folders or your own work.
  const list = useMemo(() => {
    const by = new Map<string, string>();
    for (const f of all ?? []) if (f && !UNSORTED.includes(key(f)) && !isWorkshop(f) && !by.has(key(f))) by.set(key(f), f);
    return [...by.values()];
  }, [all]);
  useEffect(() => {
    let alive = true;
    for (const f of list)
      api
        .gameIcon(f)
        .then((src) => alive && setLogo((m) => ({ ...m, [f]: !!src })))
        .catch(() => {});
    return () => {
      alive = false;
    };
  }, [list]);
  // The order is set once the logos are known, so rows don't jump while you tick them.
  const [order, setOrder] = useState<string[] | null>(null);
  const marked = settings?.notGameFolders ?? [];
  const isMarked = (f: string) => marked.some((m) => key(m) === key(f));
  useEffect(() => {
    if (order || !settings || !list.length || list.some((f) => logo[f] === undefined)) return;
    setOrder(
      [...list].sort(
        (a, b) => Number(isMarked(b)) - Number(isMarked(a)) || Number(logo[a]) - Number(logo[b]) || a.localeCompare(b, undefined, { sensitivity: "base" }),
      ),
    );
  });
  const rows = order ?? [...list].sort((a, b) => a.localeCompare(b, undefined, { sensitivity: "base" }));
  const toggle = (f: string) => {
    const next = isMarked(f) ? marked.filter((m) => key(m) !== key(f)) : [...marked, f];
    update({ notGameFolders: next }).catch((e) => setErr(errText(e)));
  };
  return createPortal(
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal notgame-modal" role="dialog" aria-modal="true" aria-label="Folders that aren't games" onClick={(e) => e.stopPropagation()}>
        <h2>Folders that aren't games</h2>
        <p className="muted" style={{ margin: 0 }}>
          Tick your own folders, like aim practice or a video you're making. The game finder won't learn from their clips or offer them
          as a game. Folders with no game logo are listed first. Nothing is moved.
        </p>
        {err && <small className="err-text">{err}</small>}
        <div className="notgame-list">
          {!all && <p className="muted">Reading your folders…</p>}
          {rows.map((f) => (
            <label key={f} className="notgame-row">
              <input type="checkbox" checked={isMarked(f)} onChange={() => toggle(f)} />
              <GameIcon name={f} size={18} />
              <span>{f}</span>
              {logo[f] === false && <small className="muted">no game logo</small>}
            </label>
          ))}
        </div>
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button className="btn primary" onClick={onClose}>
            Done
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

/**
 * Review what the picture check found: each clip with the game it looks like.
 * Sure ones come ticked; close calls come unticked with their likeliest games.
 * Nothing moves until you press Move, and the move can be undone like any other.
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
  const [settings] = useSettings();
  const [on, setOn] = useState<Set<string>>(() => new Set(list.filter((s) => s.sure).map((s) => s.path)));
  // Sure ones you unticked: not suggested again. Close calls you leave stay for later.
  const [turnedDown, setTurnedDown] = useState<Set<string>>(new Set());
  const [games, setGames] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [folderPick, setFolderPick] = useState(false);
  // New suggestions while it's open (a folder marked as not a game, say): sure ones come ticked.
  const seen = useRef(new Set(list.map((s) => s.path)));
  useEffect(() => {
    const fresh = list.filter((s) => s.sure && !seen.current.has(s.path)).map((s) => s.path);
    for (const s of list) seen.current.add(s.path);
    if (fresh.length) setOn((o) => new Set([...o, ...fresh]));
  }, [list]);
  const skip = settings?.notGameFolders ?? [];
  const known = [...new Set(clips.map((c) => c.folder).filter((f) => !UNSORTED.includes(key(f)) && !isWorkshop(f) && !skip.some((m) => key(m) === key(f))))];
  // Games named with "New game…": offered in every row's list.
  const added = Object.values(games);
  const options = (game: string) =>
    [...new Set([game, ...added, ...known])].filter((g) => key(g) !== key(NOT_A_GAME)).sort((a, b) => a.localeCompare(b, undefined, { sensitivity: "base" }));
  const sure = list.filter((s) => s.sure);
  const close = list.filter((s) => !s.sure);
  const gameOf = (s: GameSuggestion) => games[s.path] ?? s.game;
  const chosen = list.filter((s) => on.has(s.path));
  // "Not a game" for a clip already in Desktop: nothing to move, just stop suggesting it.
  const stays = (s: GameSuggestion) => key(gameOf(s)) === key(s.folder);
  const toMove = chosen.filter((s) => !stays(s));
  const moved = useCountProgress("move-progress", busy);
  const move = async () => {
    setBusy(true);
    setErr(null);
    try {
      const out = toMove.length ? await api.applyGameSort(toMove.map((s) => [s.path, gameOf(s)])) : { moved: 0, failed: [] };
      const done = [...turnedDown, ...chosen.filter(stays).map((s) => s.path)];
      if (done.length) await api.dismissGameSuggestions(done);
      onDone(out.moved, out.failed.length);
    } catch (e) {
      setErr(errText(e));
      setBusy(false);
    }
  };
  const toggle = (s: GameSuggestion) => {
    const was = on.has(s.path);
    setOn((o) => {
      const n = new Set(o);
      if (was) n.delete(s.path);
      else n.add(s.path);
      return n;
    });
    if (s.sure)
      setTurnedDown((t) => {
        const n = new Set(t);
        if (was) n.add(s.path);
        else n.delete(s.path);
        return n;
      });
  };
  const pick = (s: GameSuggestion, g: string) => {
    setGames((m) => ({ ...m, [s.path]: g }));
    setOn((o) => new Set(o).add(s.path));
    setTurnedDown((t) => {
      const n = new Set(t);
      n.delete(s.path);
      return n;
    });
  };
  const row = (s: GameSuggestion) => (
    <Row key={s.path} s={s} on={on.has(s.path)} game={gameOf(s)} games={options(s.game)} onToggle={() => toggle(s)} onGame={(g) => pick(s, g)} />
  );
  const label = toMove.length ? `Move ${toMove.length} clip${toMove.length === 1 ? "" : "s"}` : chosen.length ? "Done" : "Move";
  return createPortal(
    <div className="modal-backdrop" onClick={() => !busy && !folderPick && onClose()}>
      <div className="modal sort-modal" role="dialog" aria-modal="true" aria-label="Sort clips by game" onClick={(e) => e.stopPropagation()}>
        <h2>
          {sure.length ? `Found the game for ${sure.length} clip${sure.length === 1 ? "" : "s"}` : "Clips that might be a game you play"}
          {sure.length > 0 && close.length > 0 && <span className="muted sort-sub"> · {close.length} to check</span>}
        </h2>
        <p className="muted" style={{ margin: 0 }}>
          From what's on screen, compared with your sorted clips. Ticked clips move into that game's folder, on this PC and the NAS;
          Undo in Settings → Library puts them back. Sure ones you untick won't be suggested again. Close calls you leave stay here for
          later.
        </p>
        <div className="sort-list">
          {sure.length > 0 && close.length > 0 && <h3 className="sort-head">Sure</h3>}
          {sure.map(row)}
          {close.length > 0 && (
            <h3 className="sort-head">
              Not sure: pick the game <small className="muted">(or leave them for later)</small>
            </h3>
          )}
          {close.map(row)}
          {!list.length && <p className="muted">Nothing left to sort.</p>}
        </div>
        {err && <small className="err-text">{err}</small>}
        {busy && <ProgressBar pct={moved && moved[1] ? Math.round((moved[0] / moved[1]) * 100) : null} />}
        <div className="row">
          <button
            className="btn"
            disabled={busy}
            onClick={() => setFolderPick(true)}
            title="Your own folders (aim practice, a video project…) that the finder should never take for a game"
          >
            Folders that aren't games{skip.length ? ` (${skip.length})` : ""}…
          </button>
          <span style={{ flex: 1 }} />
          <button className="btn" disabled={busy} onClick={onClose}>
            Later
          </button>
          <button className="btn primary" disabled={busy || chosen.length === 0} onClick={move}>
            {busy ? (moved ? `Moving ${moved[0]} of ${moved[1]}…` : "Moving…") : label}
          </button>
        </div>
      </div>
      {folderPick && <NotGameFolders folders={clips.map((c) => c.folder)} onClose={() => setFolderPick(false)} />}
    </div>,
    document.body,
  );
}
