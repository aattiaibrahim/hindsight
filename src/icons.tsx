/**
 * The one icon set Hindsight uses (Lucide, ISC licence). Everything that used
 * to be an emoji, a symbol or a hand-drawn icon comes from here, so the app
 * looks the same everywhere.
 */
import {
  AlertTriangle,
  Bell,
  Check,
  ChevronRight,
  Clapperboard,
  Copy,
  Download,
  Film,
  FolderOpen,
  Folders,
  Gamepad2,
  HardDrive,
  Laptop,
  Link2,
  Scissors,
  Server,
  Settings,
  Sparkles,
  Gauge,
  X,
  type LucideProps,
} from "lucide-react";

export const Icons = {
  clips: Film,
  local: Laptop,
  synced: Server,
  totrim: Scissors,
  damaged: AlertTriangle,
  cleanup: Sparkles,
  montage: Clapperboard,
  notifications: Bell,
  performance: Gauge,
  settings: Settings,
  nas: HardDrive,
  link: Link2,
  check: Check,
  chevron: ChevronRight,
  close: X,
  folder: FolderOpen,
  folders: Folders,
  copy: Copy,
  download: Download,
  game: Gamepad2,
  warn: AlertTriangle,
} as const;

export type IconName = keyof typeof Icons;

/** An icon by name, 18px and 1.75 stroke unless told otherwise. */
export function Icon({ name, size = 18, strokeWidth = 1.75, ...rest }: { name: IconName } & LucideProps) {
  const C = Icons[name];
  return <C size={size} strokeWidth={strokeWidth} aria-hidden="true" {...rest} />;
}
