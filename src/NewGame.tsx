import { createPortal } from "react-dom";
import { GameNameInput } from "./GameNameInput";

/**
 * A new game's folder: type its name (your folders and Steam games are suggested
 * as you type). Its clips move in next: the ones already selected, or the ones you
 * pick afterwards. The folder is made when the first clip moves in.
 */
export function NewGameDialog({
  folders,
  selected,
  onPick,
  onCancel,
}: {
  /** Your folders, for the suggestions */
  folders: string[];
  /** How many clips are selected (they move in straight away) */
  selected: number;
  onPick: (name: string) => void;
  onCancel: () => void;
}) {
  return createPortal(
    <div className="modal-backdrop" onClick={onCancel}>
      <div className="modal new-game" role="dialog" aria-modal="true" aria-labelledby="new-game-title" onClick={(e) => e.stopPropagation()}>
        <h2 id="new-game-title">New game</h2>
        <p className="muted" style={{ margin: 0 }}>
          {selected
            ? `Type the game's name. The ${selected === 1 ? "selected clip moves" : `${selected} selected clips move`} into its folder, on this PC and the NAS.`
            : "Type the game's name. Next you pick the clips that go in it; its folder is made when they move in."}
        </p>
        <div className="row new-game-name">
          <GameNameInput games={folders} onPick={onPick} onCancel={onCancel} />
        </div>
        <p className="muted small" style={{ margin: 0 }}>
          Its logo is found on its own. Pick a Steam suggestion for the right spelling.
        </p>
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button className="btn" onClick={onCancel}>
            Cancel
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
