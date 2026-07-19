import type { BadgeProps } from '@arco-design/web-react';
import { Badge, Button, Message, Spin, Tooltip } from '@arco-design/web-react';
import { IconDown, IconRight } from '@arco-design/web-react/icon';
import { Checklist, Download, Earth, Right } from '@icon-park/react';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ipcBridge } from '@/common';
import { getAcpImageFileName } from '@/common/chat/acpToolCallOutput';
import type {
  NormalizedToolCall,
  NormalizedToolResultView,
  NormalizedToolResultTablePreview,
  NormalizedToolStatus,
  ToolMessage,
} from '@/common/chat/normalizeToolCall';
import { normalizeToolMessages, hasRunningToolMessages } from '@/common/chat/normalizeToolCall';
import LocalImageView from '@/renderer/components/media/LocalImageView';
import {
  fetchToolResultArtifactRead,
  fetchToolResultArtifactStream,
  type ToolResultArtifactReadContent,
} from '@/renderer/services/FileService';
import { downloadBlob, downloadFileFromPath } from '@/renderer/utils/file/download';
import './MessageToolGroupSummary.css';

const statusToBadge = (status: NormalizedToolStatus): BadgeProps['status'] => {
  switch (status) {
    case 'completed':
      return 'success';
    case 'error':
      return 'error';
    case 'running':
      return 'processing';
    case 'canceled':
      return 'default';
    case 'pending':
    default:
      return 'default';
  }
};

const formatBytes = (bytes: number): string => {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
};

const extensionFromContentType = (contentType: string): string => {
  const normalized = contentType.split(';')[0].trim().toLowerCase();
  if (normalized === 'application/json') return '.json';
  if (normalized === 'application/jsonl' || normalized === 'application/x-ndjson') return '.jsonl';
  if (normalized === 'text/markdown') return '.md';
  if (normalized === 'text/csv') return '.csv';
  if (normalized.startsWith('text/')) return '.txt';
  if (normalized === 'application/pdf') return '.pdf';
  return '.bin';
};

const safeFileName = (value: string): string => value.replace(/[^a-zA-Z0-9._-]+/g, '-').replace(/^-+|-+$/g, '');

const artifactDownloadName = (view: NormalizedToolResultView): string => {
  const ref = view.artifactRef;
  const base = safeFileName(view.title || ref?.artifactId || ref?.objectReferenceId || 'tool-result-artifact');
  const extension = extensionFromContentType(ref?.contentType || 'application/octet-stream');
  return `${base || 'tool-result-artifact'}${extension}`;
};

const formatPreviewCell = (value: unknown): string => {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
};

const recordRows = (rows: unknown[]): Array<Record<string, unknown>> =>
  rows.filter((row): row is Record<string, unknown> => Boolean(row) && typeof row === 'object' && !Array.isArray(row));

const inferColumnsFromRows = (rows: Array<Record<string, unknown>>): NormalizedToolResultTablePreview['columns'] => {
  const keys = new Set<string>();
  for (const row of rows.slice(0, 20)) {
    for (const key of Object.keys(row)) keys.add(key);
  }
  return Array.from(keys)
    .slice(0, 20)
    .map((key) => ({ key, label: key }));
};

const objectValue = (value: unknown): Record<string, unknown> | undefined =>
  value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;

const arrayValue = (value: unknown): unknown[] | undefined => (Array.isArray(value) ? value : undefined);

const numberValue = (value: unknown): number | undefined => {
  const parsed = typeof value === 'number' ? value : typeof value === 'string' ? Number(value) : Number.NaN;
  return Number.isFinite(parsed) ? parsed : undefined;
};

const fieldValue = (row: Record<string, unknown>, field: string | undefined): unknown =>
  field ? row[field] : undefined;

const normalizeSvgPoints = (
  rows: Array<Record<string, unknown>>,
  xField: string,
  yField: string,
  width: number,
  height: number,
  padding: number
): Array<{ x: number; y: number; label: string; value: number }> => {
  const values = rows
    .map((row) => ({
      xRaw: fieldValue(row, xField),
      yRaw: numberValue(fieldValue(row, yField)),
    }))
    .filter((item): item is { xRaw: unknown; yRaw: number } => item.yRaw !== undefined);
  if (values.length === 0) return [];
  const labels = values.map((item) => String(item.xRaw ?? ''));
  const minY = Math.min(0, ...values.map((item) => item.yRaw));
  const maxY = Math.max(1, ...values.map((item) => item.yRaw));
  const rangeY = maxY - minY || 1;
  const chartWidth = width - padding * 2;
  const chartHeight = height - padding * 2;
  return values.map((item, index) => ({
    x: padding + (values.length === 1 ? chartWidth / 2 : (index / (values.length - 1)) * chartWidth),
    y: padding + chartHeight - ((item.yRaw - minY) / rangeY) * chartHeight,
    label: labels[index],
    value: item.yRaw,
  }));
};

const ToolResultTablePreview: React.FC<{ table: NormalizedToolResultTablePreview }> = ({ table }) => (
  <div className='tool-result-view__table-wrap'>
    <table className='tool-result-view__table'>
      <thead>
        <tr>
          {table.columns.map((column) => (
            <th key={column.key}>{column.label}</th>
          ))}
        </tr>
      </thead>
      <tbody>
        {table.rows.map((row, rowIndex) => (
          <tr key={rowIndex}>
            {table.columns.map((column) => (
              <td key={column.key}>{formatPreviewCell(row[column.key])}</td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  </div>
);

const ToolResultChartFallback: React.FC<{ spec: Record<string, unknown> }> = ({ spec }) => {
  const data = objectValue(spec.data);
  const rows = arrayValue(data?.values)?.filter(
    (row): row is Record<string, unknown> => Boolean(row) && typeof row === 'object' && !Array.isArray(row)
  );
  const encoding = objectValue(spec.encoding);
  const xEncoding = objectValue(encoding?.x);
  const yEncoding = objectValue(encoding?.y);
  const xField = typeof xEncoding?.field === 'string' ? xEncoding.field : undefined;
  const yField = typeof yEncoding?.field === 'string' ? yEncoding.field : undefined;
  if (!rows?.length || !xField || !yField) {
    return <pre className='tool-result-view__preview'>{formatPreviewCell(spec)}</pre>;
  }

  const width = 320;
  const height = 160;
  const padding = 24;
  const mark = typeof spec.mark === 'string' ? spec.mark : objectValue(spec.mark)?.type;
  const points = normalizeSvgPoints(rows.slice(0, 24), xField, yField, width, height, padding);
  const barWidth = Math.max(6, (width - padding * 2) / Math.max(points.length, 1) - 6);
  const baseline = height - padding;
  const polyline = points.map((point) => `${point.x},${point.y}`).join(' ');

  return (
    <div className='tool-result-chart' data-testid='tool-result-chart-preview'>
      <svg viewBox={`0 0 ${width} ${height}`} role='img' aria-label='Chart preview'>
        <line x1={padding} y1={baseline} x2={width - padding} y2={baseline} className='tool-result-chart__axis' />
        <line x1={padding} y1={padding} x2={padding} y2={baseline} className='tool-result-chart__axis' />
        {mark === 'line' ? (
          <polyline points={polyline} fill='none' className='tool-result-chart__line' />
        ) : (
          points.map((point) => (
            <rect
              key={point.label}
              x={point.x - barWidth / 2}
              y={point.y}
              width={barWidth}
              height={Math.max(1, baseline - point.y)}
              className='tool-result-chart__bar'
            />
          ))
        )}
        {points.map((point) => (
          <circle
            key={`${point.label}:${point.value}`}
            cx={point.x}
            cy={point.y}
            r='2.5'
            className='tool-result-chart__point'
          >
            <title>{`${point.label}: ${point.value}`}</title>
          </circle>
        ))}
      </svg>
      <div className='tool-result-view__summary'>
        {points.length} points · x: {xField} · y: {yField}
      </div>
    </div>
  );
};

type GeoPoint = [number, number];

const collectGeoPoints = (value: unknown, points: GeoPoint[] = []): GeoPoint[] => {
  if (!Array.isArray(value)) return points;
  if (value.length >= 2 && typeof value[0] === 'number' && typeof value[1] === 'number') {
    points.push([value[0], value[1]]);
    return points;
  }
  for (const item of value) collectGeoPoints(item, points);
  return points;
};

const geoJsonCoordinates = (geoJson: Record<string, unknown>): GeoPoint[] => {
  const type = geoJson.type;
  if (type === 'FeatureCollection') {
    return (arrayValue(geoJson.features) ?? []).flatMap((feature) => {
      const geometry = objectValue(objectValue(feature)?.geometry);
      return geometry ? geoJsonCoordinates(geometry) : [];
    });
  }
  if (type === 'Feature') {
    const geometry = objectValue(geoJson.geometry);
    return geometry ? geoJsonCoordinates(geometry) : [];
  }
  return collectGeoPoints(geoJson.coordinates);
};

const hasExternalVegaDataUrl = (value: unknown): boolean => {
  const object = objectValue(value);
  if (!object) {
    if (Array.isArray(value)) return value.some(hasExternalVegaDataUrl);
    return false;
  }
  const data = objectValue(object.data);
  if (typeof data?.url === 'string' && data.url.trim()) return true;
  return Object.values(object).some(hasExternalVegaDataUrl);
};

const hasInlineVegaData = (value: Record<string, unknown>): boolean => {
  const data = objectValue(value.data);
  if (Array.isArray(data?.values)) return true;
  const datasets = objectValue(value.datasets);
  if (datasets && Object.values(datasets).some(Array.isArray)) return true;
  return false;
};

const ToolResultMapPreview: React.FC<{ geoJson: Record<string, unknown> }> = ({ geoJson }) => {
  const points = geoJsonCoordinates(geoJson).slice(0, 200);
  if (points.length === 0) {
    return <pre className='tool-result-view__preview'>{formatPreviewCell(geoJson)}</pre>;
  }
  const width = 320;
  const height = 160;
  const padding = 18;
  const minLon = Math.min(...points.map(([lon]) => lon));
  const maxLon = Math.max(...points.map(([lon]) => lon));
  const minLat = Math.min(...points.map(([, lat]) => lat));
  const maxLat = Math.max(...points.map(([, lat]) => lat));
  const lonRange = maxLon - minLon || 1;
  const latRange = maxLat - minLat || 1;
  const project = ([lon, lat]: GeoPoint): [number, number] => [
    padding + ((lon - minLon) / lonRange) * (width - padding * 2),
    height - padding - ((lat - minLat) / latRange) * (height - padding * 2),
  ];
  const path = points.map((point, index) => {
    const [x, y] = project(point);
    return `${index === 0 ? 'M' : 'L'} ${x} ${y}`;
  });

  return (
    <div className='tool-result-map' data-testid='tool-result-map-preview'>
      <svg viewBox={`0 0 ${width} ${height}`} role='img' aria-label='Map preview'>
        <rect x='1' y='1' width={width - 2} height={height - 2} rx='8' className='tool-result-map__frame' />
        {points.length > 1 && <path d={path.join(' ')} className='tool-result-map__path' />}
        {points.slice(0, 80).map((point, index) => {
          const [x, y] = project(point);
          return (
            <circle key={`${point[0]}:${point[1]}:${index}`} cx={x} cy={y} r='2.5' className='tool-result-map__point' />
          );
        })}
      </svg>
      <div className='tool-result-view__summary'>
        {points.length} coordinates · lon {minLon.toFixed(3)}..{maxLon.toFixed(3)} · lat {minLat.toFixed(3)}..
        {maxLat.toFixed(3)}
      </div>
    </div>
  );
};

type VegaEmbedResult = {
  view?: {
    finalize?: () => void;
  };
};

const ToolResultVegaLitePreview: React.FC<{ spec: Record<string, unknown> }> = ({ spec }) => {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [runtimeState, setRuntimeState] = useState<'loading' | 'ready' | 'fallback'>('loading');
  const canUseRuntime = hasInlineVegaData(spec) && !hasExternalVegaDataUrl(spec);

  useEffect(() => {
    if (!canUseRuntime) {
      setRuntimeState('fallback');
      return;
    }
    const container = containerRef.current;
    if (!container) return;
    let disposed = false;
    let embedResult: VegaEmbedResult | null = null;
    container.replaceChildren();
    setRuntimeState('loading');

    void import('vega-embed')
      .then(async (module) => {
        const embed = module.default;
        embedResult = (await embed(container, spec, {
          actions: false,
          renderer: 'svg',
          tooltip: true,
        })) as VegaEmbedResult;
        if (!disposed) setRuntimeState('ready');
      })
      .catch(() => {
        if (!disposed) setRuntimeState('fallback');
      });

    return () => {
      disposed = true;
      embedResult?.view?.finalize?.();
      container.replaceChildren();
    };
  }, [canUseRuntime, spec]);

  return (
    <div className='tool-result-runtime'>
      <div
        ref={containerRef}
        className='tool-result-runtime__chart'
        data-testid='tool-result-vega-runtime'
        data-runtime-state={runtimeState}
      />
      <div hidden={runtimeState === 'ready'}>
        <ToolResultChartFallback spec={spec} />
      </div>
    </div>
  );
};

type MapLibreModule = {
  default?: {
    Map?: new (options: Record<string, unknown>) => MapLibreMapInstance;
  };
  Map?: new (options: Record<string, unknown>) => MapLibreMapInstance;
};

type MapLibreMapInstance = {
  on: (event: string, handler: () => void) => unknown;
  fitBounds?: (bounds: [[number, number], [number, number]], options?: Record<string, unknown>) => unknown;
  remove: () => void;
};

const geoJsonBounds = (geoJson: Record<string, unknown>): [[number, number], [number, number]] | undefined => {
  const points = geoJsonCoordinates(geoJson);
  if (points.length === 0) return undefined;
  const minLon = Math.min(...points.map(([lon]) => lon));
  const maxLon = Math.max(...points.map(([lon]) => lon));
  const minLat = Math.min(...points.map(([, lat]) => lat));
  const maxLat = Math.max(...points.map(([, lat]) => lat));
  return [
    [minLon, minLat],
    [maxLon, maxLat],
  ];
};

const mapLibreStyleForGeoJson = (geoJson: Record<string, unknown>) => ({
  version: 8,
  sources: {
    'tool-result': {
      type: 'geojson',
      data: geoJson,
    },
  },
  layers: [
    {
      id: 'background',
      type: 'background',
      paint: { 'background-color': '#f8fafc' },
    },
    {
      id: 'polygon-fill',
      type: 'fill',
      source: 'tool-result',
      filter: ['==', '$type', 'Polygon'],
      paint: { 'fill-color': '#165dff', 'fill-opacity': 0.12 },
    },
    {
      id: 'polygon-line',
      type: 'line',
      source: 'tool-result',
      filter: ['==', '$type', 'Polygon'],
      paint: { 'line-color': '#165dff', 'line-width': 1.2, 'line-opacity': 0.72 },
    },
    {
      id: 'line',
      type: 'line',
      source: 'tool-result',
      filter: ['==', '$type', 'LineString'],
      paint: { 'line-color': '#165dff', 'line-width': 2.2, 'line-opacity': 0.82 },
    },
    {
      id: 'points',
      type: 'circle',
      source: 'tool-result',
      filter: ['==', '$type', 'Point'],
      paint: {
        'circle-color': '#00b42a',
        'circle-radius': 4,
        'circle-stroke-color': '#ffffff',
        'circle-stroke-width': 1,
      },
    },
  ],
});

const ToolResultMapLibrePreview: React.FC<{ geoJson: Record<string, unknown> }> = ({ geoJson }) => {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [runtimeState, setRuntimeState] = useState<'loading' | 'ready' | 'fallback'>('loading');

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    let disposed = false;
    let map: MapLibreMapInstance | null = null;
    container.replaceChildren();
    setRuntimeState('loading');

    void import('maplibre-gl')
      .then((module: MapLibreModule) => {
        const MapConstructor = module.default?.Map ?? module.Map;
        if (!MapConstructor) throw new Error('maplibre Map constructor is unavailable');
        map = new MapConstructor({
          container,
          style: mapLibreStyleForGeoJson(geoJson),
          attributionControl: false,
          interactive: true,
        });
        map.on('load', () => {
          if (disposed) return;
          const bounds = geoJsonBounds(geoJson);
          if (bounds) {
            map?.fitBounds?.(bounds, { padding: 24, maxZoom: 12, duration: 0 });
          }
          setRuntimeState('ready');
        });
      })
      .catch(() => {
        if (!disposed) setRuntimeState('fallback');
      });

    return () => {
      disposed = true;
      map?.remove();
      container.replaceChildren();
    };
  }, [geoJson]);

  return (
    <div className='tool-result-runtime'>
      <div
        ref={containerRef}
        className='tool-result-runtime__map'
        data-testid='tool-result-maplibre-runtime'
        data-runtime-state={runtimeState}
      />
      <div hidden={runtimeState === 'ready'}>
        <ToolResultMapPreview geoJson={geoJson} />
      </div>
    </div>
  );
};

const ToolResultArtifactContentPreview: React.FC<{
  content: ToolResultArtifactReadContent;
  fallbackColumns?: NormalizedToolResultTablePreview['columns'];
}> = ({ content, fallbackColumns }) => {
  if (content.kind === 'json_rows') {
    const rows = recordRows(content.rows);
    const columns = fallbackColumns?.length ? fallbackColumns : inferColumnsFromRows(rows);
    return (
      <div>
        <div className='tool-result-view__summary'>
          Loaded rows {content.offset + 1}-{content.offset + rows.length} of {content.total_rows}
        </div>
        <ToolResultTablePreview table={{ columns, rows }} />
      </div>
    );
  }
  if (content.kind === 'json_value') {
    return <pre className='tool-result-view__preview'>{formatPreviewCell(content.value)}</pre>;
  }
  if (content.kind === 'text' || content.kind === 'text_byte_range') {
    return <pre className='tool-result-view__preview'>{content.text}</pre>;
  }
  return (
    <pre className='tool-result-view__preview'>
      {content.content_type} · {formatBytes(content.size_bytes)}
    </pre>
  );
};

const ToolResultViewPreview: React.FC<{
  view: NormalizedToolResultView;
  artifactContent?: ToolResultArtifactReadContent;
}> = ({ view, artifactContent }) => {
  if (artifactContent) {
    return <ToolResultArtifactContentPreview content={artifactContent} fallbackColumns={view.tablePreview?.columns} />;
  }
  if (view.tablePreview) return <ToolResultTablePreview table={view.tablePreview} />;
  if (view.chartPreview) return <ToolResultVegaLitePreview spec={view.chartPreview} />;
  if (view.mapPreview) return <ToolResultMapLibrePreview geoJson={view.mapPreview} />;
  if (view.previewText) return <pre className='tool-result-view__preview'>{view.previewText}</pre>;
  if (view.summary) return <pre className='tool-result-view__summary'>{view.summary}</pre>;
  return null;
};

const ToolItemDetail: React.FC<{ item: NormalizedToolCall }> = ({ item }) => {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const [showTechnical, setShowTechnical] = useState(false);
  const [fullItem, setFullItem] = useState<NormalizedToolCall | null>(null);
  const [loadingFull, setLoadingFull] = useState(false);
  const [loadError, setLoadError] = useState(false);
  const [downloadingArtifact, setDownloadingArtifact] = useState<string | null>(null);
  const [loadingArtifactPreview, setLoadingArtifactPreview] = useState<string | null>(null);
  const [artifactPreviewErrors, setArtifactPreviewErrors] = useState<Record<string, boolean>>({});
  const [artifactPreviewContent, setArtifactPreviewContent] = useState<Record<string, ToolResultArtifactReadContent>>(
    {}
  );
  const displayItem = fullItem ?? item;
  const hasViews = Boolean(displayItem.views?.length);
  const hasDetail =
    displayItem.input ||
    displayItem.output ||
    displayItem.inputFields?.length ||
    displayItem.resultSummary ||
    item.truncated ||
    item.imagePath ||
    hasViews;
  const [messageApi, messageContext] = Message.useMessage();
  const handleDownloadImage = useCallback(
    async (path: string) => {
      try {
        await downloadFileFromPath(path, getAcpImageFileName(path));
        messageApi.success(t('acp.image.download_success'));
      } catch (error) {
        console.error('[MessageToolGroupSummary] Failed to download image:', error);
        messageApi.error(t('acp.image.download_error'));
      }
    },
    [messageApi, t]
  );

  const handleDownloadArtifact = useCallback(
    async (view: NormalizedToolResultView) => {
      const objectReferenceId = view.artifactRef?.objectReferenceId;
      if (!objectReferenceId) return;
      setDownloadingArtifact(objectReferenceId);
      try {
        const bootstrap = await ipcBridge.workbench.bootstrap.invoke();
        const tenantId = bootstrap.auth.tenant_id;
        const response = await fetchToolResultArtifactStream(tenantId, objectReferenceId);
        const blob = await response.blob();
        downloadBlob(blob, artifactDownloadName(view));
        messageApi.success(t('messages.downloadSuccess'));
      } catch (error) {
        console.error('[MessageToolGroupSummary] Failed to download artifact:', error);
        messageApi.error(t('messages.downloadFailed'));
      } finally {
        setDownloadingArtifact(null);
      }
    },
    [messageApi, t]
  );

  const handleLoadArtifactPreview = useCallback(
    async (objectReferenceId: string) => {
      if (loadingArtifactPreview === objectReferenceId || artifactPreviewContent[objectReferenceId]) return;
      setLoadingArtifactPreview(objectReferenceId);
      setArtifactPreviewErrors((current) => ({ ...current, [objectReferenceId]: false }));
      try {
        const bootstrap = await ipcBridge.workbench.bootstrap.invoke();
        const tenantId = bootstrap.auth.tenant_id;
        const result = await fetchToolResultArtifactRead(tenantId, objectReferenceId, { offset: 0, limit: 500 });
        setArtifactPreviewContent((current) => ({
          ...current,
          [objectReferenceId]: result.content,
        }));
      } catch (error) {
        console.error('[MessageToolGroupSummary] Failed to load artifact preview:', error);
        setArtifactPreviewErrors((current) => ({ ...current, [objectReferenceId]: true }));
      } finally {
        setLoadingArtifactPreview(null);
      }
    },
    [artifactPreviewContent, loadingArtifactPreview]
  );

  const loadFullItem = async () => {
    if (!item.truncated || fullItem || loadingFull || !item.conversationId || !item.messageId) return;
    setLoadingFull(true);
    setLoadError(false);
    try {
      const message = await ipcBridge.database.getConversationMessage.invoke({
        conversation_id: item.conversationId,
        message_id: item.messageId,
      });
      const next = normalizeToolMessages([message as ToolMessage]).find((candidate) => candidate.key === item.key);
      if (next) setFullItem(next);
    } catch {
      setLoadError(true);
    } finally {
      setLoadingFull(false);
    }
  };

  const toggleExpanded = () => {
    const nextExpanded = !expanded;
    setExpanded(nextExpanded);
    if (nextExpanded) void loadFullItem();
  };

  return (
    <div className='flex flex-col'>
      {messageContext}
      <div className='flex flex-row color-#86909C gap-12px items-center'>
        <Badge status={statusToBadge(item.status)} className={item.status === 'running' ? 'badge-breathing' : ''} />
        <span
          className={
            'flex-1 min-w-0' +
            (expanded ? ' break-all' : ' truncate') +
            (hasDetail ? ' cursor-pointer hover:color-#4E5969' : '')
          }
          onClick={hasDetail ? toggleExpanded : undefined}
        >
          <span className='font-medium text-13px'>{displayItem.name}</span>
          {displayItem.description && displayItem.description !== displayItem.name && (
            <span className='m-l-4px opacity-80 text-13px'>{displayItem.description}</span>
          )}
        </span>
        {hasDetail && (
          <span className='flex-shrink-0 cursor-pointer hover:color-#4E5969 transition-colors' onClick={toggleExpanded}>
            {expanded ? <IconDown style={{ fontSize: 12 }} /> : <IconRight style={{ fontSize: 12 }} />}
          </span>
        )}
      </div>
      {item.browser && (
        <div className='browser-tool-summary' data-testid='browser-tool-summary-card'>
          <Earth theme='outline' size='14' className='browser-tool-summary__icon' />
          <div className='browser-tool-summary__content'>
            <div className='browser-tool-summary__title'>{item.browser.title || item.browser.action || item.name}</div>
            {item.browser.url && <div className='browser-tool-summary__url'>{item.browser.url}</div>}
            <div className='browser-tool-summary__meta'>
              {item.browser.action ? `Action: ${item.browser.action}` : 'Browser session'}
              {typeof item.browser.elementCount === 'number'
                ? ` · ${item.browser.elementCount} interactive elements`
                : ''}
              {item.browser.closed ? ' · closed' : ''}
            </div>
          </div>
        </div>
      )}
      {expanded && hasDetail && (
        <div className='tool-detail-panel m-l-20px m-t-4px'>
          {loadingFull && <div className='tool-detail-label'>Loading...</div>}
          {loadError && <div className='tool-detail-label'>Failed to load full output</div>}
          {Boolean(displayItem.views?.length) && (
            <div className='tool-detail-section'>
              <div className='tool-detail-label'>Views</div>
              <div className='tool-result-views'>
                {displayItem.views.map((view, index) => {
                  const ref = view.artifactRef;
                  const downloadKey = ref?.objectReferenceId;
                  return (
                    <div
                      className='tool-result-view'
                      data-testid='tool-result-view'
                      key={`${view.kind}:${view.title ?? index}:${downloadKey ?? ''}`}
                    >
                      <div className='tool-result-view__header'>
                        <span className='tool-result-view__title'>{view.title || view.kind}</span>
                        <span className='tool-result-view__kind'>{view.kind}</span>
                        {ref && <span className='tool-result-view__size'>{formatBytes(ref.sizeBytes)}</span>}
                        {downloadKey && (
                          <>
                            <Button
                              data-testid={`tool-result-artifact-preview-${downloadKey}`}
                              size='mini'
                              type='secondary'
                              loading={loadingArtifactPreview === downloadKey}
                              disabled={Boolean(artifactPreviewContent[downloadKey])}
                              onClick={() => void handleLoadArtifactPreview(downloadKey)}
                            >
                              Preview
                            </Button>
                            <Button
                              data-testid={`tool-result-artifact-download-${downloadKey}`}
                              aria-label={t('common.download')}
                              size='mini'
                              type='secondary'
                              loading={downloadingArtifact === downloadKey}
                              icon={<Download theme='outline' size='12' />}
                              onClick={() => void handleDownloadArtifact(view)}
                            />
                          </>
                        )}
                      </div>
                      {downloadKey && artifactPreviewErrors[downloadKey] && (
                        <div className='tool-result-view__summary'>Failed to load artifact preview</div>
                      )}
                      <ToolResultViewPreview
                        view={view}
                        artifactContent={downloadKey ? artifactPreviewContent[downloadKey] : undefined}
                      />
                    </div>
                  );
                })}
              </div>
            </div>
          )}
          {Boolean(displayItem.inputFields?.length) && (
            <div className='tool-detail-section'>
              <div className='tool-detail-label'>Request</div>
              <div className='tool-display-fields'>
                {displayItem.inputFields?.map((field) => (
                  <div className='tool-display-field' key={field.key}>
                    <span className='tool-display-field__label'>{field.label}</span>
                    <span className='tool-display-field__value'>{field.value}</span>
                  </div>
                ))}
              </div>
            </div>
          )}
          {displayItem.resultSummary && !hasViews && (
            <div className='tool-detail-section'>
              <div className='tool-detail-label'>Result</div>
              <div className='tool-display-result'>{displayItem.resultSummary}</div>
            </div>
          )}
          {(displayItem.input || displayItem.output) && (
            <button className='tool-technical-toggle' type='button' onClick={() => setShowTechnical(!showTechnical)}>
              {showTechnical ? 'Hide technical details' : 'Technical details'}
            </button>
          )}
          {showTechnical && displayItem.input && (
            <div className='tool-detail-section'>
              <div className='tool-detail-label'>Raw input</div>
              <pre className='tool-detail-content'>{displayItem.input}</pre>
            </div>
          )}
          {showTechnical && displayItem.output && (
            <div className='tool-detail-section'>
              <div className='tool-detail-label'>Raw output</div>
              <pre className='tool-detail-content'>{displayItem.output}</pre>
            </div>
          )}
        </div>
      )}
      {item.imagePath && (
        <div className='group relative m-l-20px m-t-8px overflow-hidden rounded border bg-1 p-2 max-w-280px'>
          <LocalImageView
            src={item.imagePath}
            alt={getAcpImageFileName(item.imagePath)}
            className='max-w-full max-h-320px object-contain rounded'
          />
          <Tooltip content={t('acp.image.download')}>
            <Button
              aria-label={t('acp.image.download_aria')}
              className='!absolute right-10px top-10px !h-28px !w-28px !p-0 opacity-0 shadow-sm transition-opacity group-hover:opacity-90 focus:opacity-100'
              type='secondary'
              size='mini'
              shape='circle'
              icon={<Download theme='outline' size='14' />}
              onClick={() => void handleDownloadImage(item.imagePath)}
            />
          </Tooltip>
        </div>
      )}
    </div>
  );
};

const MessageToolGroupSummary: React.FC<{ messages: ToolMessage[] }> = ({ messages }) => {
  const hasRunning = hasRunningToolMessages(messages);
  const [showMore, setShowMore] = useState(hasRunning);

  useEffect(() => {
    if (hasRunning) setShowMore(true);
  }, [hasRunning]);

  const tools = useMemo(() => normalizeToolMessages(messages), [messages]);

  return (
    <div className='tool-group-summary'>
      <div className='tool-group-summary__header' onClick={() => setShowMore(!showMore)}>
        <span className='tool-group-summary__icon'>
          {hasRunning ? <Spin size={12} /> : <Checklist theme='outline' size='14' />}
        </span>
        <span className='tool-group-summary__label'>View Steps {tools.length > 0 ? `· ${tools.length}` : ''}</span>
        <span className={`tool-group-summary__arrow${showMore ? ' tool-group-summary__arrow--open' : ''}`}>
          <Right theme='outline' size='12' />
        </span>
      </div>
      {showMore && (
        <div className='tool-group-summary__body'>
          {tools.map((item) => (
            <ToolItemDetail key={item.key} item={item} />
          ))}
        </div>
      )}
    </div>
  );
};

export default React.memo(MessageToolGroupSummary);
