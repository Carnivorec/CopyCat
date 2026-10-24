import "@fontsource-variable/schibsted-grotesk";
import "@fontsource-variable/noto-sans-sc";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "@fontsource/ibm-plex-mono/600.css";
import "./styles.css";

import { createRoot } from "react-dom/client";
import { App } from "./App";
import { VIEW } from "./api";
import { SendApp } from "./Transfer";

document.addEventListener("contextmenu", (e) => {
  const t = e.target as HTMLElement | null;
  if (!t?.closest("input, textarea, [contenteditable=true]")) e.preventDefault();
});

createRoot(document.getElementById("root")!).render(VIEW === "send" ? <SendApp /> : <App />);
