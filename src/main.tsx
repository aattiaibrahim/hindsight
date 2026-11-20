import { getCurrentWindow } from "@tauri-apps/api/window";
import React from "react";
import ReactDOM from "react-dom/client";
import App, { useEscapeToHide } from "./App";
// Geist, bundled (latin only, variable weight) so the app looks the same on every PC.
import "@fontsource-variable/geist/wght.css";
import "@fontsource-variable/geist-mono/wght.css";
import "./App.css";
import { Flyout } from "./Flyout";
import { applyCachedAppearance } from "./hooks";
import { ErrorBoundary } from "./ErrorBoundary";

function FlyoutRoot() {
  useEscapeToHide();
  return <Flyout />;
}

async function start() {
  applyCachedAppearance();
  // In a plain browser during development, use fake sample data.
  if (import.meta.env.DEV) (await import("./devMock")).installDevMock();
  const isFlyout = getCurrentWindow().label === "flyout";
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <ErrorBoundary>{isFlyout ? <FlyoutRoot /> : <App />}</ErrorBoundary>
    </React.StrictMode>,
  );
}

start();
