export interface ToolDisplayField {
  key: string;
  label: string;
  value: string;
  sensitive?: boolean;
}

export interface ToolDisplayProjection {
  summary?: string;
  inputFields?: ToolDisplayField[];
  resultSummary?: string;
}

const SENSITIVE_KEY = /api[_-]?key|authorization|password|secret|token|credential|private[_-]?key|cookie/i;
const PREFERRED_SUMMARY_KEYS = ['query', 'pattern', 'path', 'file_path', 'url', 'command', 'address', 'name'];
const RESULT_SUMMARY_KEYS = ['output_summary', 'error_summary', 'message', 'summary', 'status', 'result'];

const recordValue = (value: unknown): Record<string, unknown> | undefined =>
  value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;

const unwrapInput = (value: unknown): unknown => {
  let current = value;
  for (let depth = 0; depth < 3; depth += 1) {
    const record = recordValue(current);
    if (!record) break;
    if (record.arguments && Object.keys(record).length === 1) {
      current = record.arguments;
      continue;
    }
    if (record.input && Object.keys(record).length === 1) {
      current = record.input;
      continue;
    }
    const args = Array.isArray(record.args) ? record.args : undefined;
    const kwargs = recordValue(record.kwargs);
    if (kwargs && args?.length === 0 && Object.keys(record).every((key) => key === 'args' || key === 'kwargs')) {
      current = kwargs;
      continue;
    }
    break;
  }
  return current;
};

export const humanizeToolLabel = (value: string): string => {
  const words = value
    .replace(/^(tool|mcp)[_:.-]+/i, '')
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .replace(/[_:.-]+/g, ' ')
    .trim();
  if (!words) return value;
  return words.charAt(0).toUpperCase() + words.slice(1);
};

const scalarText = (value: unknown): string | undefined => {
  if (typeof value === 'string') return value.trim() || undefined;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  if (Array.isArray(value)) {
    if (value.length === 0) return undefined;
    if (value.every((item) => ['string', 'number', 'boolean'].includes(typeof item))) {
      const preview = value.slice(0, 5).map(String).join(', ');
      return value.length > 5 ? `${preview} … (${value.length} items)` : preview;
    }
    return `${value.length} items`;
  }
  const record = recordValue(value);
  if (record) {
    const count = Object.keys(record).length;
    return count > 0 ? `${count} fields` : undefined;
  }
  return undefined;
};

const trimText = (value: string, limit = 280): string =>
  value.length <= limit ? value : `${value.slice(0, limit).trimEnd()}…`;

const fieldsFromInput = (input: unknown): ToolDisplayField[] | undefined => {
  const value = unwrapInput(input);
  const record = recordValue(value);
  if (!record) {
    const text = scalarText(value);
    return text ? [{ key: 'details', label: 'Details', value: trimText(text) }] : undefined;
  }
  const fields = Object.entries(record)
    .filter(([, entry]) => entry !== null && entry !== undefined && entry !== '')
    .slice(0, 8)
    .map(([key, entry]) => {
      const sensitive = SENSITIVE_KEY.test(key);
      return {
        key,
        label: humanizeToolLabel(key),
        value: sensitive ? 'Hidden' : trimText(scalarText(entry) ?? 'Complex value'),
        sensitive,
      };
    });
  return fields.length > 0 ? fields : undefined;
};

const summaryFromInput = (input: unknown): string | undefined => {
  const record = recordValue(unwrapInput(input));
  if (!record) return scalarText(input);
  for (const key of PREFERRED_SUMMARY_KEYS) {
    const value = scalarText(record[key]);
    if (value && !SENSITIVE_KEY.test(key)) return trimText(value, 140);
  }
  const first = Object.entries(record).find(([key, value]) => !SENSITIVE_KEY.test(key) && scalarText(value));
  return first ? trimText(scalarText(first[1]) ?? '', 140) : undefined;
};

const tryParseJson = (value: string): unknown => {
  const trimmed = value.trim();
  if (!trimmed || (!trimmed.startsWith('{') && !trimmed.startsWith('['))) return value;
  try {
    return JSON.parse(trimmed);
  } catch {
    return value;
  }
};

const summaryFromResult = (output: unknown): string | undefined => {
  const value = typeof output === 'string' ? tryParseJson(output) : output;
  if (typeof value === 'string') return value.trim() ? trimText(value.trim(), 600) : undefined;
  const record = recordValue(value);
  if (record) {
    for (const key of RESULT_SUMMARY_KEYS) {
      if (SENSITIVE_KEY.test(key)) continue;
      const summary = scalarText(record[key]);
      if (summary) return trimText(summary, 600);
    }
    const count = Object.keys(record).length;
    return count > 0 ? `Completed with ${count} result fields` : undefined;
  }
  if (Array.isArray(value)) return `${value.length} result items`;
  return scalarText(value);
};

export function buildToolDisplayProjection(input: unknown, output?: unknown): ToolDisplayProjection {
  return {
    summary: summaryFromInput(input),
    inputFields: fieldsFromInput(input),
    resultSummary: summaryFromResult(output),
  };
}
