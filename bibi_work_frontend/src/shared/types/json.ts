export type JsonPrimitive = string | number | boolean | null;
export type JsonValue = JsonPrimitive | JsonValue[] | { [key: string]: JsonValue };
export type JsonRecord = { [key: string]: JsonValue };

export function asRecord(value: JsonValue | undefined): JsonRecord {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

export function stringFromJson(value: JsonValue | undefined, fallback = ""): string {
  return typeof value === "string" ? value : fallback;
}
