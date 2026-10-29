import ReactDOM from "react-dom/client";
import { App } from "./app/App";
import { initTheme } from "./app/theme";
import { applyPlatform } from "./app/platform";
import "./styles/app.css";

// Before the first render, so the window never flashes the wrong theme.
initTheme();
applyPlatform();

ReactDOM.createRoot(document.getElementById("root")!).render(<App />);
