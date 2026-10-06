import React from "react";
import ReactDOM from "react-dom/client";
import "./i18n";
import "./styles.css";
import App from "./App";

async function start() {
  // In a plain browser (`pnpm dev` without Tauri) run against recorded sample data.
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
    const { installMocks } = await import("./dev/mock");
    installMocks();
  }
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}

void start();
