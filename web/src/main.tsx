import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "@/AppV2";
import "./index.css";
import "xterm/css/xterm.css";

const root = document.getElementById("root");
if (!root) throw new Error("#root missing");
createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
