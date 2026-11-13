import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { inTauri, useMockBackend } from "./services/api";
import "./styles.css";

async function start() {
  // Outside the desktop shell (plain `vite dev` in a browser) show sample data.
  if (!inTauri && import.meta.env.DEV) await useMockBackend();
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}
void start();
