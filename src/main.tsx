import ReactDOM from "react-dom/client";
import { App } from "./app/App";
import { initTheme } from "./app/theme";
import { applyPlatform } from "./app/platform";
import { usePrefs } from "./app/prefs";
import { applyTerminalScheme } from "./features/terminal/themes";
import "./styles/app.css";

// Before the first render, so the window never flashes the wrong theme.
initTheme();
applyPlatform();
// The terminal surface follows the chosen scheme, live.
applyTerminalScheme(usePrefs.getState().terminalScheme);
usePrefs.subscribe((p, prev) => {
  if (p.terminalScheme !== prev.terminalScheme) applyTerminalScheme(p.terminalScheme);
});

ReactDOM.createRoot(document.getElementById("root")!).render(<App />);
