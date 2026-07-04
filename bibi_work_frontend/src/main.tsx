import React from "react";
import ReactDOM from "react-dom/client";
import { PlatformProviders } from "./app/providers";
import { App } from "./app/App";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <PlatformProviders>
      <App />
    </PlatformProviders>
  </React.StrictMode>
);
