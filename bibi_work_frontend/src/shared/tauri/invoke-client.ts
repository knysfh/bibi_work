import { invoke } from "@tauri-apps/api/core";

export interface InvokeClient {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  isTauriRuntime(): boolean;
}

export function createInvokeClient(): InvokeClient {
  return {
    invoke: (command, args) => invoke(command, args),
    isTauriRuntime
  };
}

export function isTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}
