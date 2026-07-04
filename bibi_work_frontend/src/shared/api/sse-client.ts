import type { JsonValue } from "../types/json";
import { ContractError } from "./errors";
import type { HttpClient } from "./http-client";

export interface SseMessage {
  id?: string;
  event?: string;
  data: string;
}

export async function postSse(
  http: HttpClient,
  path: string,
  body: unknown,
  onMessage: (message: SseMessage) => void,
  signal?: AbortSignal,
  query?: Record<string, string | number | boolean | null | undefined>
): Promise<void> {
  const response = await http.postRaw(path, body, {
    signal,
    query,
    auth: true
  });
  await readSseResponse(response, onMessage);
}

export async function getSse(
  http: HttpClient,
  path: string,
  onMessage: (message: SseMessage) => void,
  signal?: AbortSignal,
  query?: Record<string, string | number | boolean | null | undefined>
): Promise<void> {
  const response = await http.getRaw(path, {
    signal,
    query,
    auth: true
  });
  await readSseResponse(response, onMessage);
}

async function readSseResponse(
  response: Response,
  onMessage: (message: SseMessage) => void
): Promise<void> {
  if (!response.body) {
    throw new ContractError("SSE response does not expose a readable stream", response);
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";

  for (;;) {
    const { done, value } = await reader.read();
    buffer += decoder.decode(value ?? new Uint8Array(), { stream: !done });
    const chunks = buffer.split("\n\n");
    buffer = chunks.pop() ?? "";
    for (const chunk of chunks) {
      const message = parseSseChunk(chunk);
      if (message) {
        onMessage(message);
      }
    }
    if (done) {
      if (buffer.trim()) {
        const message = parseSseChunk(buffer);
        if (message) {
          onMessage(message);
        }
      }
      break;
    }
  }
}

export function parseSseJson<T extends JsonValue = JsonValue>(message: SseMessage): T | null {
  if (!message.data || message.data === "[DONE]") {
    return null;
  }
  return JSON.parse(message.data) as T;
}

function parseSseChunk(chunk: string): SseMessage | null {
  const message: SseMessage = { data: "" };
  for (const line of chunk.split("\n")) {
    if (!line || line.startsWith(":")) {
      continue;
    }
    const separator = line.indexOf(":");
    const field = separator === -1 ? line : line.slice(0, separator);
    const value = separator === -1 ? "" : line.slice(separator + 1).trimStart();
    if (field === "id") {
      message.id = value;
    } else if (field === "event") {
      message.event = value;
    } else if (field === "data") {
      message.data += message.data ? `\n${value}` : value;
    }
  }
  return message.data || message.id || message.event ? message : null;
}
