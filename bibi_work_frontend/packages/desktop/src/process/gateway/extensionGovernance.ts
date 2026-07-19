import type { IHubAgentItem } from '../../common/types/agent/hub';

function dataFromBackendEnvelope(body: unknown): unknown {
  return body && typeof body === 'object' && !Array.isArray(body) && 'data' in body
    ? (body as { data?: unknown }).data
    : body;
}

function objectRecords(value: unknown): Record<string, unknown>[] {
  if (!Array.isArray(value)) return [];
  return value.filter(
    (item): item is Record<string, unknown> => Boolean(item) && typeof item === 'object' && !Array.isArray(item)
  );
}

export function extractHubExtensions(body: unknown): IHubAgentItem[] {
  const data = dataFromBackendEnvelope(body);
  if (!Array.isArray(data)) {
    throw new Error('HUB_EXTENSIONS_RESPONSE_INVALID');
  }
  return data as IHubAgentItem[];
}

export function replaceBackendData(body: unknown, data: unknown): Record<string, unknown> {
  if (body && typeof body === 'object' && !Array.isArray(body) && 'data' in body) {
    return { ...(body as Record<string, unknown>), data };
  }
  return { success: true, data };
}

function extensionItemKey(pathname: string, item: Record<string, unknown>, index: number): string {
  if (pathname === '/api/extensions') return String(item.name ?? index);
  if (pathname === '/api/extensions/settings-tabs')
    return `${String(item.extensionName ?? '')}:${String(item.id ?? index)}`;
  return `${String(item._extensionName ?? item.extensionName ?? '')}:${String(item.id ?? item.name ?? item.type ?? item.plugin_id ?? index)}`;
}

export function mergeExtensionData(pathname: string, backendBodyValue: unknown, localData: unknown): unknown {
  const existing = dataFromBackendEnvelope(backendBodyValue);
  if (Array.isArray(existing)) {
    // Rust owns governance. After sync, backend arrays are the allow-list; do not
    // re-add local contributions that Rust filtered out.
    return objectRecords(existing);
  }
  if (Array.isArray(localData)) {
    return objectRecords(localData);
  }
  if (pathname === '/api/extensions/i18n' && existing && typeof existing === 'object' && !Array.isArray(existing)) {
    return { ...(existing as Record<string, unknown>), ...(localData as Record<string, unknown>) };
  }
  if (
    pathname === '/api/extensions/agent-activity' &&
    existing &&
    typeof existing === 'object' &&
    !Array.isArray(existing)
  ) {
    const existingAgents = Array.isArray((existing as Record<string, unknown>).agents)
      ? ((existing as Record<string, unknown>).agents as unknown[])
      : [];
    const localAgents =
      localData &&
      typeof localData === 'object' &&
      !Array.isArray(localData) &&
      Array.isArray((localData as Record<string, unknown>).agents)
        ? ((localData as Record<string, unknown>).agents as unknown[])
        : [];
    return { ...(existing as Record<string, unknown>), agents: [...existingAgents, ...localAgents] };
  }
  return localData;
}

export function isExtensionStaticAssetAllowed(backendBodyValue: unknown, extensionName: string): boolean {
  const normalizedName = extensionName.trim();
  if (!normalizedName) return false;
  return objectRecords(dataFromBackendEnvelope(backendBodyValue)).some((item) => {
    const itemName = String(item.name ?? item.extension_name ?? '').trim();
    if (itemName !== normalizedName) return false;
    const installStatus = String(item.install_status ?? item.status ?? '');
    return (
      item.enabled === true &&
      item.installed === true &&
      (installStatus === 'installed' || installStatus === 'update_available')
    );
  });
}

function channelPluginKey(item: Record<string, unknown>, index: number): string {
  return String(item.plugin_id ?? item.id ?? item.type ?? index);
}

function channelPluginContractRecord(item: Record<string, unknown>): Record<string, unknown> {
  const record = { ...item };
  delete record.extensionName;
  delete record.key;
  delete record._extensionName;
  delete record._source;
  return record;
}

function allowedChannelPluginRecords(allowedExtensionPluginsBody: unknown): Map<string, Record<string, unknown>> {
  const allowed = new Map<string, Record<string, unknown>>();
  objectRecords(dataFromBackendEnvelope(allowedExtensionPluginsBody)).forEach((item, index) => {
    allowed.set(channelPluginKey(item, index), channelPluginContractRecord(item));
  });
  return allowed;
}

export function mergeChannelPlugins(
  backendBodyValue: unknown,
  localPlugins: Record<string, unknown>[],
  allowedExtensionPluginsBody: unknown
): Record<string, unknown>[] {
  const merged = new Map<string, Record<string, unknown>>();
  const allowedExtensionPlugins = allowedChannelPluginRecords(allowedExtensionPluginsBody);
  const backendPlugins = objectRecords(dataFromBackendEnvelope(backendBodyValue));

  backendPlugins.forEach((record, index) => {
    merged.set(channelPluginKey(record, index), record);
  });

  localPlugins.forEach((record, index) => {
    const key = channelPluginKey(record, index);
    const allowedRecord = allowedExtensionPlugins.get(key);
    if (!allowedRecord) return;
    const backendRecord = merged.get(key);
    const authorityRecord = backendRecord ?? allowedRecord;
    merged.set(key, {
      ...record,
      ...allowedRecord,
      ...authorityRecord,
      extension_meta: {
        ...(record.extension_meta as Record<string, unknown> | undefined),
        ...(allowedRecord.extension_meta as Record<string, unknown> | undefined),
        ...(authorityRecord.extension_meta as Record<string, unknown> | undefined),
      },
    });
  });

  return [...merged.values()];
}

export const extensionGovernanceTestInternals = {
  extensionItemKey,
};
