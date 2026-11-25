import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { api } from "./api";
import { Monitor } from "lucide-react";
import { Icon } from "./icons";


/**
 * A game's logo: its own program icon when the game is installed or has been
 * played (or one you picked), else a plain gamepad on grey. "Desktop" (everything
 * that isn't a game) gets a screen instead, unless you picked a logo for it.
 */
export function GameIcon({ name, size = 18 }: { name: string; size?: number }) {
  const [src, setSrc] = useState<string | null>(null);
  const [version, setVersion] = useState(0);
  // A logo picked (or reset) anywhere: show the new one.
  useEffect(() => {
    const un = listen("icons-changed", () => setVersion((v) => v + 1));
    return () => {
      un.then((f) => f());
    };
  }, []);
  useEffect(() => {
    let alive = true;
    setSrc(null);
    if (name && name !== "All") api.gameIcon(name).then((s) => alive && setSrc(s ? `${s}${s.includes("?") ? "&" : "?"}v=${version}` : null));
    return () => {
      alive = false;
    };
  }, [name, version]);
  if (src) return <img className="game-icon" src={src} alt="" width={size} height={size} style={{ width: size, height: size }} />;
  if (name.toLowerCase().replace(/[^a-z0-9]/g, "") === "desktop")
    return (
      <span className="game-icon none desktop" aria-hidden="true" style={{ width: size, height: size }}>
        <Monitor size={Math.round(size * 0.7)} strokeWidth={2} />
      </span>
    );
  return (
    <span className="game-icon none" aria-hidden="true" style={{ width: size, height: size }}>
      <Icon name="game" size={Math.round(size * 0.68)} strokeWidth={2} />
    </span>
  );
}
