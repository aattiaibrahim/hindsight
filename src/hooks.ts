import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { api, ClipFile, Settings, Status, UpdateState, NOT_NEEDED, markOnScreen, want as wantThumb } from "./api";

export function useUpdate() {
  const [u, setU] = useState<UpdateState | null>(null);
  useEffect(() => {
    api.getUpdateState().then(setU).catch(() => {});
    const un = listen<UpdateState>("update-state", (e) =>
      setU((prev) => ({ ...e.payload, current: prev?.current || e.payload.current })),
    );
    return () => {
      un.then((f) => f());
    };
  }, []);
  return u;
}

export function useStatus() {
  const [status, setStatus] = useState<Status | null>(null);
  useEffect(() => {
    api.getStatus().then(setStatus).catch(() => {});
    const un = listen<Status>("engine-status", (e) => setStatus(e.payload));
    return () => {
      un.then((f) => f());
    };
  }, []);
  return status;
}

export function useSettings() {
  const [settings, setLocal] = useState<Settings | null>(null);
  useEffect(() => {
    api.getSettings().then(setLocal).catch(() => {});
    const un = listen<Settings>("settings-changed", (e) => setLocal(e.payload));
    return () => {
      un.then((f) => f());
    };
  }, []);
  const update = useCallback(async (patch: Partial<Settings>) => {
    const current = await api.getSettings();
    const next = { ...current, ...patch };
    setLocal(next);
    await api.setSettings(next);
  }, []);
  return [settings, update] as const;
}

/** The clip list, refreshed when a clip is saved or the clips folder changes. */
export function useClips() {
  const [clips, setClips] = useState<ClipFile[] | null>(null);
  const refresh = useCallback(() => {
    api.listClips().then(setClips).catch(() => setClips([]));
  }, []);
  useEffect(() => {
    refresh();
    const a = listen("clip-saved", refresh);
    const b = listen("settings-changed", refresh);
    const c = listen("clips-changed", refresh);
    window.addEventListener("focus", refresh);
    return () => {
      a.then((f) => f());
      b.then((f) => f());
      c.then((f) => f());
      window.removeEventListener("focus", refresh);
    };
  }, [refresh]);
  return [clips, refresh] as const;
}

/**
 * Has this element come near the screen yet? Stays true once it has, so a
 * card loads its thumbnail once and keeps it. `root` is the scrolling box
 * (found by selector) and `margin` how far ahead to start.
 */
/**
 * The list this element scrolls in. Its own, not the first one on the page: every list
 * you open stays loaded, so "the first .lib-page" could be another, hidden list, and
 * cards watching it never counted as near the screen (no thumbnail, no details).
 */
function scroller(el: Element, selector: string) {
  return el.closest(selector) ?? document.querySelector(selector);
}

export function useNearView<T extends Element>(rootSelector = ".lib-page", margin = "900px 0px") {
  const ref = useRef<T>(null);
  const [near, setNear] = useState(false);
  useEffect(() => {
    if (near) return;
    const el = ref.current;
    if (!el) return;
    if (typeof IntersectionObserver === "undefined") return setNear(true);
    const io = new IntersectionObserver(
      (es) => {
        if (es.some((e) => e.isIntersecting)) {
          setNear(true);
          io.disconnect();
        }
      },
      { root: scroller(el, rootSelector), rootMargin: margin },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [near, rootSelector, margin]);
  return [ref, near] as const;
}

/**
 * A clip's thumbnail. One already loaded this session shows on the first frame
 * (going back to the library doesn't blank them all); otherwise it's fetched
 * once `want` is true (cards pass "near the screen").
 */
/**
 * Tell the request queue while this element is really on screen (not just near it),
 * so its thumbnail and details go before the ones it asked for ahead of the scroll.
 */
export function useOnScreen<T extends Element>(ref: React.RefObject<T | null>, keys: string[], rootSelector = ".lib-page") {
  const joined = keys.join("\n");
  useEffect(() => {
    const el = ref.current;
    const list = joined.split("\n");
    if (!el || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver((es) => markOnScreen(list, es.some((e) => e.isIntersecting)), { root: scroller(el, rootSelector) });
    io.observe(el);
    return () => {
      io.disconnect();
      markOnScreen(list, false);
    };
  }, [ref, joined, rootSelector]);
}

/** How long a card waits before asking again for a thumbnail or details that failed. */
export const RETRY_MS = [2500, 8000, 20000];

export function useThumb(path: string, key: string | number = "", want = true) {
  const [src, setSrc] = useState<string | null>(() => (path ? api.thumbNow(path, String(key)) : null));
  useEffect(() => {
    let alive = true;
    const now = path ? api.thumbNow(path, String(key)) : null;
    setSrc(now);
    if (!path || now || !want) return;
    const k = "t:" + path + String(key);
    wantThumb(k, 1);
    let retry = 0;
    const load = (tries: number) =>
      api
        .thumbnail(path, String(key), true)
        .then((s) => alive && setSrc(s))
        .catch((e) => {
          if (!alive) return;
          // Still on screen but told "not needed" (a mix-up over who wants it): ask again now.
          // Otherwise (a slow drive, a busy NAS) try again a few times, waiting longer each time.
          if (String(e).includes(NOT_NEEDED)) retry = window.setTimeout(() => load(tries), 50);
          else if (tries < RETRY_MS.length) retry = window.setTimeout(() => load(tries + 1), RETRY_MS[tries]);
        });
    load(0);
    return () => {
      alive = false;
      clearTimeout(retry);
      wantThumb(k, -1); // left the screen: a request still waiting its turn is dropped
    };
  }, [path, key, want]);
  return src;
}

/** Theme, accent and motion as attributes on <html>, so the CSS can switch
 *  colours instantly (no re-render). Also remembered locally so the next
 *  launch paints in the right theme before settings have loaded. */
export function useAppearance(settings: Settings | null) {
  useEffect(() => {
    if (!settings) return;
    const look = { theme: settings.theme ?? "tinted", accent: settings.accent ?? "winblue", motion: settings.reduceMotion ? "reduce" : "full" };
    applyAppearance(look);
    try {
      localStorage.setItem("rewind.appearance", JSON.stringify(look));
    } catch {
      /* only a start-up nicety */
    }
  }, [settings?.theme, settings?.accent, settings?.reduceMotion]); // eslint-disable-line react-hooks/exhaustive-deps
  // "Follow Windows": Windows' accent, checked again whenever Hindsight comes back to the front.
  useEffect(() => {
    if (settings?.accent !== "system") return;
    const load = () =>
      api
        .systemAccent()
        .then((c) => {
          if (!c) return;
          document.documentElement.style.setProperty("--system-accent", c);
          try {
            localStorage.setItem("rewind.systemAccent", c);
          } catch {
            /* only a start-up nicety */
          }
        })
        .catch(() => {});
    load();
    window.addEventListener("focus", load);
    return () => window.removeEventListener("focus", load);
  }, [settings?.accent]);
}

export function applyAppearance(look: { theme: string; accent: string; motion: string }) {
  const el = document.documentElement;
  el.dataset.theme = look.theme;
  el.dataset.accent = look.accent;
  el.dataset.motion = look.motion;
}

/** Paint in the last-used theme before anything loads. */
export function applyCachedAppearance() {
  try {
    const v = localStorage.getItem("rewind.appearance");
    if (v) applyAppearance(JSON.parse(v));
    const sys = localStorage.getItem("rewind.systemAccent");
    if (sys) document.documentElement.style.setProperty("--system-accent", sys);
  } catch {
    /* first launch or storage unavailable: defaults */
  }
}
