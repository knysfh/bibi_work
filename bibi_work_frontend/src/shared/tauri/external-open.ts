import { invoke } from "@tauri-apps/api/core";
import { isTauriRuntime } from "./invoke-client";

export async function openControlledExternalUrl(url: string): Promise<void> {
  if (isTauriRuntime()) {
    await invoke("auth_open_external_browser", { url });
    return;
  }

  window.open(url, "_blank", "noopener,noreferrer");
}
