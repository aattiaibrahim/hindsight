import { useEffect, useRef, useState } from "react";
import { api, CropRect, Monitor } from "./api";
import { CropBox, cropPixels, fitRatio, isWhole, WHOLE } from "./CropBox";

/** How often the preview refreshes while it's on screen. */
const EVERY_MS = 400;

const SHAPES: [string, number | null][] = [
  ["Free", null],
  ["16:9", 16 / 9],
  ["4:3", 4 / 3],
  ["1:1", 1],
  ["9:16", 9 / 16],
];

/**
 * Pick the part of the screen to record, on a live preview of it (like OBS).
 * The preview only runs while this is open and the window is showing, and the
 * app refuses it while a game is running, so it never costs in-game FPS.
 */
export function RecordingArea({
  area,
  screen,
  onSave,
}: {
  area: CropRect | null;
  screen: Monitor | undefined;
  onSave: (a: CropRect | null) => void;
}) {
  const [frame, setFrame] = useState<string | null>(null);
  const [paused, setPaused] = useState<"" | "game" | "error">("");
  const [draft, setDraft] = useState<CropRect>(area ?? WHOLE);
  const [ratio, setRatio] = useState<number | null>(null);
  const urlRef = useRef<string | null>(null);

  // A saved change elsewhere (or a reset) shows up in the box.
  useEffect(() => setDraft(area ?? WHOLE), [area?.x, area?.y, area?.w, area?.h]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    let stop = false;
    let timer = 0;
    const tick = async () => {
      if (stop) return;
      if (document.hidden) {
        timer = window.setTimeout(tick, 1000);
        return;
      }
      try {
        const bytes = await api.screenPreview();
        if (stop) return;
        const url = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
        if (urlRef.current) URL.revokeObjectURL(urlRef.current);
        urlRef.current = url;
        setFrame(url);
        setPaused("");
        timer = window.setTimeout(tick, EVERY_MS);
      } catch (e) {
        if (stop) return;
        // A game started: stop grabbing the screen, check again now and then.
        setPaused(String(e).includes("game") ? "game" : "error");
        timer = window.setTimeout(tick, 3000);
      }
    };
    tick();
    return () => {
      stop = true;
      clearTimeout(timer);
      if (urlRef.current) URL.revokeObjectURL(urlRef.current);
      urlRef.current = null;
    };
  }, []);

  const sw = screen?.width || 1920;
  const sh = screen?.height || 1080;
  const ar = sw / sh;
  const [pw, ph] = cropPixels(draft, sw, sh);
  const changed = JSON.stringify(isWhole(draft) ? null : draft) !== JSON.stringify(isWhole(area) ? null : area);

  return (
    <div className="rec-area">
      <div className="rec-area-view" style={{ aspectRatio: `${sw} / ${sh}` }}>
        {frame ? <img src={frame} alt="" draggable={false} /> : <div className="rec-area-blank" />}
        <CropBox value={draft} onChange={setDraft} ratio={ratio} videoAr={ar} editing label={isWhole(draft) ? "Whole screen" : `${pw} × ${ph}`} />
        {paused && (
          <div className="rec-area-paused">
            {paused === "game" ? "Preview paused while a game is running, so it doesn't cost FPS." : "Couldn't show the screen right now."}
          </div>
        )}
      </div>
      <div className="row rec-area-tools">
        <span className="transport-pill crop-tools" role="group" aria-label="Area shape">
          {SHAPES.map(([name, r]) => (
            <button
              key={name}
              className={`btn pill ${ratio === r ? "on" : ""}`}
              aria-pressed={ratio === r}
              onClick={() => {
                setRatio(r);
                if (r) setDraft(fitRatio(r, ar));
              }}
            >
              {name}
            </button>
          ))}
        </span>
        <span style={{ flex: 1 }} />
        <button
          className="btn"
          disabled={isWhole(draft) && isWhole(area)}
          onClick={() => {
            setDraft(WHOLE);
            setRatio(null);
            onSave(null);
          }}
        >
          Whole screen
        </button>
        <button className="btn primary" disabled={!changed} onClick={() => onSave(isWhole(draft) ? null : draft)}>
          Use this area
        </button>
      </div>
    </div>
  );
}
