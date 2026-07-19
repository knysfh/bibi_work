import type { IMessageAcpToolCall, IMessageToolCall, IMessageToolGroup } from './chatLib';
import { getAcpImagePath } from './acpToolCallOutput';
import type { ToolResultArtifactRef, ToolResultView } from '@/common/types/platform/acpTypes';
import { buildToolDisplayProjection, type ToolDisplayField } from './toolDisplayProjection';

export type NormalizedToolStatus = 'pending' | 'running' | 'completed' | 'error' | 'canceled';

export interface NormalizedToolCall {
  key: string;
  name: string;
  status: NormalizedToolStatus;
  description?: string;
  input?: string;
  output?: string;
  truncated?: boolean;
  messageId?: string;
  conversationId?: string;
  imagePath?: string;
  views?: NormalizedToolResultView[];
  inputFields?: ToolDisplayField[];
  resultSummary?: string;
  browser?: NormalizedBrowserToolResult;
}

export interface NormalizedBrowserToolResult {
  action?: string;
  sessionId?: string;
  profile?: string;
  url?: string;
  title?: string;
  elementCount?: number;
  closed?: boolean;
}

export interface NormalizedToolResultArtifactRef {
  artifactId: string;
  objectReferenceId?: string;
  contentType: string;
  contentHash: string;
  sizeBytes: number;
}

export interface NormalizedToolResultView {
  kind: ToolResultView['kind'];
  title?: string;
  summary?: string;
  artifactRef?: NormalizedToolResultArtifactRef;
  tablePreview?: NormalizedToolResultTablePreview;
  chartPreview?: Record<string, unknown>;
  mapPreview?: Record<string, unknown>;
  previewText?: string;
}

export interface NormalizedToolResultTablePreview {
  columns: NormalizedToolResultTableColumn[];
  rows: Array<Record<string, unknown>>;
}

export interface NormalizedToolResultTableColumn {
  key: string;
  label: string;
  type?: string;
}

const decodeEscapedUnicode = (value: string): string =>
  value.replace(/\\u([0-9a-fA-F]{4})/g, (_match, hex: string) => String.fromCharCode(Number.parseInt(hex, 16)));

const formatValue = (value: unknown): string => {
  if (typeof value === 'string') return decodeEscapedUnicode(value);
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
};

const trimSummary = (value: string, limit = 1200): string => {
  if (value.length <= limit) return value;
  return `${value.slice(0, limit)}...`;
};

const normalizeArtifactRef = (ref: ToolResultArtifactRef | undefined): NormalizedToolResultArtifactRef | undefined => {
  if (!ref || typeof ref.artifact_id !== 'string' || !ref.artifact_id) return undefined;
  return {
    artifactId: ref.artifact_id,
    objectReferenceId: typeof ref.object_reference_id === 'string' ? ref.object_reference_id : undefined,
    contentType: ref.content_type,
    contentHash: ref.content_hash,
    sizeBytes: ref.size_bytes,
  };
};

const summarizeToolResultView = (view: ToolResultView): string | undefined => {
  switch (view.kind) {
    case 'table':
      return `${view.rows_preview.length} preview row${view.rows_preview.length === 1 ? '' : 's'}, ${view.columns.length} column${view.columns.length === 1 ? '' : 's'}`;
    case 'chart':
      return view.spec_kind === 'vega_lite' ? 'Vega-Lite chart preview' : undefined;
    case 'map':
      return view.format === 'geojson' ? 'GeoJSON map data' : undefined;
    case 'json':
      return trimSummary(formatValue(view.value_preview));
    case 'file_diff':
      return view.files.map((file) => file.path || file.file_name || 'changes.diff').join('\n');
    case 'markdown':
      return trimSummary(view.text);
    case 'artifact':
      return view.artifact_ref.content_type;
    default:
      return undefined;
  }
};

const normalizeTablePreview = (view: ToolResultView): NormalizedToolResultTablePreview | undefined => {
  if (view.kind !== 'table') return undefined;
  return {
    columns: view.columns.slice(0, 20).map((column) => ({
      key: column.key,
      label: column.label || column.key,
      type: column.type,
    })),
    rows: view.rows_preview.slice(0, 50),
  };
};

const normalizeChartPreview = (view: ToolResultView): Record<string, unknown> | undefined => {
  if (view.kind !== 'chart') return undefined;
  return view.spec;
};

const normalizeMapPreview = (view: ToolResultView): Record<string, unknown> | undefined => {
  if (view.kind !== 'map') return undefined;
  return view.data_preview;
};

const previewTextForToolResultView = (view: ToolResultView): string | undefined => {
  switch (view.kind) {
    case 'chart':
      return undefined;
    case 'map':
      return undefined;
    case 'json':
      return trimSummary(formatValue(view.value_preview));
    case 'file_diff':
      return trimSummary(
        view.files
          .map((file) => {
            const name = file.path || file.file_name || 'changes.diff';
            return `# ${name}\n${file.file_diff}`;
          })
          .join('\n\n')
      );
    case 'markdown':
      return trimSummary(view.text);
    case 'artifact':
      return view.artifact_ref.content_type;
    default:
      return undefined;
  }
};

const getToolResultViewArtifactRef = (view: ToolResultView): NormalizedToolResultArtifactRef | undefined => {
  if (view.kind === 'artifact') return normalizeArtifactRef(view.artifact_ref);
  if ('data_ref' in view) return normalizeArtifactRef(view.data_ref);
  return undefined;
};

const normalizeToolResultViews = (views: ToolResultView[] | undefined): NormalizedToolResultView[] | undefined => {
  if (!Array.isArray(views) || views.length === 0) return undefined;
  const normalized = views.map((view) => ({
    kind: view.kind,
    title: view.title,
    summary: summarizeToolResultView(view),
    artifactRef: getToolResultViewArtifactRef(view),
    tablePreview: normalizeTablePreview(view),
    chartPreview: normalizeChartPreview(view),
    mapPreview: normalizeMapPreview(view),
    previewText: previewTextForToolResultView(view),
  }));
  return normalized.length > 0 ? normalized : undefined;
};

// ===== tool_group → NormalizedToolCall[] =====

function normalizeToolGroupStatus(status: string): NormalizedToolStatus {
  switch (status) {
    case 'Success':
      return 'completed';
    case 'Error':
      return 'error';
    case 'Canceled':
      return 'canceled';
    case 'Pending':
      return 'pending';
    case 'Executing':
    case 'Confirming':
    default:
      return 'running';
  }
}

const getResultDisplayText = (
  result_display: IMessageToolGroup['content'][0]['result_display']
): string | undefined => {
  if (!result_display) return undefined;
  if (typeof result_display === 'string') return decodeEscapedUnicode(result_display);
  if ('file_diff' in result_display) return result_display.file_diff;
  if ('img_url' in result_display) return result_display.relative_path || result_display.img_url;
  if ('kind' in result_display && result_display.kind === 'browser') return formatValue(result_display);
  return undefined;
};

const normalizeBrowserToolResult = (toolName: string, value: unknown): NormalizedBrowserToolResult | undefined => {
  if (!toolName.startsWith('browser_')) return undefined;
  let candidate = value;
  if (typeof candidate === 'string') {
    try {
      candidate = JSON.parse(candidate);
    } catch {
      return undefined;
    }
  }
  const result =
    candidate && typeof candidate === 'object' && !Array.isArray(candidate)
      ? (candidate as Record<string, unknown>)
      : undefined;
  if (!result || result.kind !== 'browser') return undefined;
  return {
    action: typeof result.action === 'string' ? result.action : undefined,
    sessionId: typeof result.session_id === 'string' ? result.session_id : undefined,
    profile: typeof result.profile === 'string' ? result.profile : undefined,
    url: typeof result.url === 'string' ? result.url : undefined,
    title: typeof result.title === 'string' ? result.title : undefined,
    elementCount: typeof result.element_count === 'number' ? result.element_count : undefined,
    closed: typeof result.closed === 'boolean' ? result.closed : undefined,
  };
};

export function normalizeToolGroup(message: IMessageToolGroup): NormalizedToolCall[] {
  if (!Array.isArray(message.content)) return [];
  return message.content.map(({ name, call_id, description, confirmationDetails, status, result_display }) => {
    let desc = typeof description === 'string' ? description.slice(0, 100) : '';
    const type = confirmationDetails?.type;
    if (type === 'edit') desc = confirmationDetails.file_name;
    if (type === 'exec') desc = confirmationDetails.command;
    if (type === 'info') desc = confirmationDetails.urls?.join(';') || confirmationDetails.title;
    if (type === 'mcp') desc = confirmationDetails.server_name + ':' + confirmationDetails.tool_name;

    let input: string | undefined;
    let projectionInput: unknown;
    if (confirmationDetails) {
      const { title: _title, type: _type, ...rest } = confirmationDetails;
      if (Object.keys(rest).length) {
        projectionInput = rest;
        input = formatValue(rest);
      }
    } else if (description) {
      projectionInput = description;
      input = description;
    }

    const output = getResultDisplayText(result_display);
    const projection = buildToolDisplayProjection(projectionInput, output);

    return {
      key: call_id,
      name,
      status: normalizeToolGroupStatus(status),
      description: desc,
      input,
      output,
      inputFields: projection.inputFields,
      resultSummary: projection.resultSummary,
      browser: normalizeBrowserToolResult(name, result_display),
    };
  });
}

// ===== acp_tool_call → NormalizedToolCall =====

function normalizeAcpStatus(status: string): NormalizedToolStatus {
  switch (status) {
    case 'completed':
      return 'completed';
    case 'failed':
      return 'error';
    case 'in_progress':
      return 'running';
    case 'pending':
    default:
      return 'pending';
  }
}

const buildParamSummary = (kind: string, rawInput?: Record<string, unknown>): string | undefined => {
  if (!rawInput) return undefined;

  if (kind === 'read' || kind === 'edit') {
    return (rawInput.file_path as string) || (rawInput.path as string) || (rawInput.file_name as string);
  }
  if (kind === 'execute') {
    return rawInput.command as string;
  }
  if (kind === 'search' || kind === 'grep') {
    const parts: string[] = [];
    if (rawInput.pattern) parts.push(`"${rawInput.pattern}"`);
    if (rawInput.path) parts.push(`in ${rawInput.path}`);
    else if (rawInput.glob) parts.push(`in ${rawInput.glob}`);
    return parts.length > 0 ? parts.join(' ') : undefined;
  }
  if (kind === 'glob') {
    const parts: string[] = [];
    if (rawInput.pattern) parts.push(`${rawInput.pattern}`);
    if (rawInput.path) parts.push(`in ${rawInput.path}`);
    return parts.length > 0 ? parts.join(' ') : undefined;
  }
  if (kind === 'write') {
    return (rawInput.file_path as string) || (rawInput.path as string);
  }

  for (const key of ['file_path', 'command', 'path', 'pattern', 'query', 'url']) {
    if (rawInput[key] && typeof rawInput[key] === 'string') return rawInput[key] as string;
  }
  return undefined;
};

type AcpToolCallUpdateCompat = IMessageAcpToolCall['content']['update'] & {
  session_update?: string;
  raw_input?: Record<string, unknown>;
};

type AcpToolCallContentCompat = IMessageAcpToolCall['content'] & {
  _compact?: {
    truncated?: boolean;
    original_size?: number;
    preview_chars?: number;
  };
  update?: AcpToolCallUpdateCompat;
};

export function normalizeAcpToolCall(message: IMessageAcpToolCall): NormalizedToolCall | undefined {
  const content = message.content as AcpToolCallContentCompat | undefined;
  const update = content?.update;
  if (!update) return undefined;

  const rawInput = update.rawInput ?? update.raw_input;
  const rawOutput = update.rawOutput ?? update.raw_output;
  const input = rawInput ? formatValue(rawInput) : undefined;

  let output: string | undefined;
  if (Array.isArray(update.content) && update.content.length) {
    output = update.content
      .map((item) => {
        if (item.type === 'content' && item.content?.text) return item.content.text;
        if (item.type === 'diff' && 'path' in item) return `[diff] ${item.path}`;
        return '';
      })
      .filter(Boolean)
      .join('\n');
  }
  if (!output) {
    output = rawOutput?.output_summary || rawOutput?.error_summary;
  }
  if (output) output = decodeEscapedUnicode(output);

  const keyParam = buildParamSummary(update.kind, rawInput);
  const projection = buildToolDisplayProjection(rawInput, output ?? rawOutput);

  return {
    key: update.tool_call_id,
    name: update.title,
    status: normalizeAcpStatus(update.status),
    description: keyParam || (rawInput?.command as string) || update.kind,
    input,
    output: output ? decodeEscapedUnicode(output) : output,
    truncated: content?._compact?.truncated === true,
    messageId: message.id,
    conversationId: message.conversation_id,
    imagePath: getAcpImagePath(update),
    views: normalizeToolResultViews(rawOutput?.views),
    inputFields: projection.inputFields,
    resultSummary: projection.resultSummary,
    browser: normalizeBrowserToolResult(update.title, rawOutput?.browser ?? rawOutput?.output_summary),
  };
}

// ===== tool_call → NormalizedToolCall =====

function normalizeToolCallStatus(status?: string): NormalizedToolStatus {
  switch (status) {
    case 'completed':
      return 'completed';
    case 'error':
      return 'error';
    case 'running':
      return 'running';
    default:
      return 'pending';
  }
}

export function normalizeToolCall(message: IMessageToolCall): NormalizedToolCall | undefined {
  const { call_id, name, status, input, output, args, description } = message.content;
  if (!call_id) return undefined;

  const displayInput = input
    ? formatValue(input)
    : args && Object.keys(args).length > 0
      ? formatValue(args)
      : undefined;
  const sourceInput = input ?? args;
  const projection = buildToolDisplayProjection(sourceInput, output);

  return {
    key: call_id,
    name,
    status: normalizeToolCallStatus(status),
    description: description || projection.summary,
    input: displayInput,
    output: output ? decodeEscapedUnicode(output) : output,
    inputFields: projection.inputFields,
    resultSummary: projection.resultSummary,
  };
}

// ===== Unified entry =====

export type ToolMessage = IMessageToolGroup | IMessageAcpToolCall | IMessageToolCall;

export function normalizeToolMessages(messages: ToolMessage[]): NormalizedToolCall[] {
  return messages
    .flatMap((m) => {
      if (m.type === 'tool_group') return normalizeToolGroup(m);
      if (m.type === 'acp_tool_call') return normalizeAcpToolCall(m);
      if (m.type === 'tool_call') return normalizeToolCall(m);
      return undefined;
    })
    .filter((item): item is NormalizedToolCall => item !== undefined);
}

export function hasRunningToolMessages(messages: ToolMessage[]): boolean {
  return messages.some((m) => {
    if (m.type === 'tool_group') {
      return Array.isArray(m.content) && m.content.some((t) => normalizeToolGroupStatus(t.status) === 'running');
    }
    if (m.type === 'acp_tool_call') {
      return m.content?.update && normalizeAcpStatus(m.content.update.status) === 'running';
    }
    if (m.type === 'tool_call') {
      return normalizeToolCallStatus(m.content?.status) === 'running';
    }
    return false;
  });
}
