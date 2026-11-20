import { Component, ReactNode } from "react";
import { logUi } from "./api";

/**
 * A mistake in one part of the interface shows a message with a way back,
 * instead of a black window. What went wrong goes to the detailed log.
 */
export class ErrorBoundary extends Component<{ children: ReactNode; label?: string }, { error: Error | null }> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: { componentStack?: string | null }) {
    const where = (info.componentStack ?? "").trim().split("\n")[0]?.trim() ?? "";
    logUi(`crash in ${this.props.label ?? "the app"}: ${error.message} ${where}`);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="crash" role="alert">
        <strong>Something went wrong{this.props.label ? ` in ${this.props.label}` : ""}.</strong>
        <small className="muted">It's been written to the detailed log (Settings → About). Your clips are fine.</small>
        <span className="row">
          <button className="btn primary" onClick={() => this.setState({ error: null })}>
            Try again
          </button>
          <button className="btn" onClick={() => location.reload()}>
            Reload Hindsight
          </button>
        </span>
      </div>
    );
  }
}
