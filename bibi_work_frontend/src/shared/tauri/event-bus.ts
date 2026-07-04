import { listen } from "@tauri-apps/api/event";
import type { RunEvent } from "../contracts/platform";
import type { JsonValue } from "../types/json";
import { isTauriRuntime } from "./invoke-client";

export type AppEvent =
  | { type: "auth.callback"; payload: { code: string; state: string; error?: string } }
  | { type: "run.event"; payload: RunEvent }
  | { type: "approval.changed"; payload: JsonValue }
  | { type: "file.changed"; payload: JsonValue }
  | { type: "workflow.node.changed"; payload: JsonValue }
  | { type: "localExec.event"; payload: JsonValue }
  | { type: "session.revoked"; payload: { sessionId?: string; deviceId?: string } };

export type AppEventHandler = (event: AppEvent) => void;

export class AppEventBus extends EventTarget {
  publish(event: AppEvent): void {
    this.dispatchEvent(new CustomEvent(event.type, { detail: event }));
  }

  subscribe(type: AppEvent["type"], handler: AppEventHandler): () => void {
    const listener = (event: Event) => {
      handler((event as CustomEvent<AppEvent>).detail);
    };
    this.addEventListener(type, listener);
    return () => this.removeEventListener(type, listener);
  }
}

export async function bindTauriEvents(bus: AppEventBus): Promise<() => void> {
  if (!isTauriRuntime()) {
    return () => undefined;
  }

  const unlistenAuth = await listen<{ code: string; state: string; error?: string }>(
    "auth.callback",
    (event) => {
      bus.publish({ type: "auth.callback", payload: event.payload });
    }
  );
  const unlistenLocalExec = await listen<JsonValue>("localExec.event", (event) => {
    bus.publish({ type: "localExec.event", payload: event.payload });
  });
  const unlistenSession = await listen<{ sessionId?: string; deviceId?: string }>(
    "session.revoked",
    (event) => {
      bus.publish({ type: "session.revoked", payload: event.payload });
    }
  );

  return () => {
    unlistenAuth();
    unlistenLocalExec();
    unlistenSession();
  };
}
