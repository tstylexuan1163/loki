import ReactDOM from "react-dom/client";
import { App } from "./app/App";
import { initTheme, useTheme } from "./app/theme";
import { applyChrome } from "./app/chrome";
import { applyPlatform } from "./app/platform";
import { usePrefs } from "./app/prefs";
import { applyTerminalScheme } from "./features/terminal/themes";
import "./styles/app.css";

// Before the first render, so the window never flashes the wrong theme.
initTheme();
applyPlatform();
// The terminal surface follows the chosen scheme, live, and so does the chrome
// when it is set to follow it. The theme store's own apply runs first; the
// chrome may then override day/night for a scheme-derived chrome.
const refreshChrome = () => {
  const p = usePrefs.getState();
  applyChrome(p.chromeStyle, p.terminalScheme, useTheme.getState().resolved);
};
applyTerminalScheme(usePrefs.getState().terminalScheme);
refreshChrome();
usePrefs.subscribe((p, prev) => {
  if (p.terminalScheme !== prev.terminalScheme) applyTerminalScheme(p.terminalScheme);
  if (p.terminalScheme !== prev.terminalScheme || p.chromeStyle !== prev.chromeStyle) refreshChrome();
});
// Unconditional: the theme store re-applies `data-theme` even when nothing
// changed (it does on rehydration), which would undo a scheme-derived mode.
useTheme.subscribe(refreshChrome);

ReactDOM.createRoot(document.getElementById("root")!).render(<App />);
