import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import App from "./App";
import { Popover } from "./popover/Popover";
import "./index.css";
import "./native.css";

// One bundle, two windows: the menubar popover and the full app window. Both
// use the native look; the popover is also fully transparent (frosted by macOS).
const isPopover = getCurrentWindow().label === "popover";

// Surface frontend errors in the app log.
const report = (m: string) => void invoke("js_log", { message: m }).catch(() => {});
window.addEventListener("error", (e) => report(`${e.message} @ ${e.filename}:${e.lineno}`));
window.addEventListener("unhandledrejection", (e) => report(`unhandled: ${String(e.reason)}`));
document.documentElement.classList.add("native", isPopover ? "popover" : "main");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{isPopover ? <Popover /> : <App />}</React.StrictMode>,
);
