import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";

// In a plain browser there's no Tauri backend, so fake one (dev only).
const ready = import.meta.env.DEV && !("__TAURI_INTERNALS__" in window) ? import("./devMock") : Promise.resolve();

ready.then(() =>
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  ),
);
