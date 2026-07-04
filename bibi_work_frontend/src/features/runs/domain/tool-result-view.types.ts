import type { JsonRecord, JsonValue } from "../../../shared/types/json";

export interface ToolResultArtifactRef {
  artifactId: string;
  objectReferenceId?: string;
  contentType: string;
  contentHash: string;
  sizeBytes: number;
}

export interface TableToolResultColumn {
  key: string;
  label?: string;
  type?: "string" | "number" | "boolean" | "datetime" | "currency";
}

export interface TableToolResultView {
  kind: "table";
  title?: string;
  columns: TableToolResultColumn[];
  rowsPreview: JsonRecord[];
  dataRef?: ToolResultArtifactRef;
}

export interface ChartToolResultView {
  kind: "chart";
  title?: string;
  specKind: "vega_lite";
  spec: JsonRecord;
  dataRef?: ToolResultArtifactRef;
}

export interface MapToolResultView {
  kind: "map";
  title?: string;
  format: "geojson";
  dataRef: ToolResultArtifactRef;
  dataPreview?: JsonRecord;
  styleRef?: string;
}

export interface JsonToolResultView {
  kind: "json";
  title?: string;
  valuePreview: JsonValue;
  dataRef?: ToolResultArtifactRef;
}

export interface FileDiffToolResultView {
  kind: "file_diff";
  title?: string;
  files: JsonValue[];
}

export interface MarkdownToolResultView {
  kind: "markdown";
  title?: string;
  text: string;
}

export interface ArtifactToolResultView {
  kind: "artifact";
  title?: string;
  artifactRef: ToolResultArtifactRef;
}

export type ToolResultView =
  | TableToolResultView
  | ChartToolResultView
  | MapToolResultView
  | JsonToolResultView
  | FileDiffToolResultView
  | MarkdownToolResultView
  | ArtifactToolResultView;
