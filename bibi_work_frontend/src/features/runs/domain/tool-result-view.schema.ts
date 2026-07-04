import { z } from "zod";
import { jsonValueSchema } from "../../../shared/contracts/platform";
import type { JsonValue } from "../../../shared/types/json";
import type { ToolResultView } from "./tool-result-view.types";

const titleSchema = z.string().max(160).optional();

const artifactRefSchema = z
  .object({
    artifact_id: z.string().min(1).max(128),
    object_reference_id: z.string().uuid().nullable().optional(),
    content_type: z.string().min(3).max(128),
    content_hash: z.string().startsWith("sha256:"),
    size_bytes: z.number().nonnegative()
  })
  .transform((value) => ({
    artifactId: value.artifact_id,
    objectReferenceId: value.object_reference_id ?? undefined,
    contentType: value.content_type,
    contentHash: value.content_hash,
    sizeBytes: value.size_bytes
  }));

const tableViewSchema = z
  .object({
    kind: z.literal("table"),
    title: titleSchema,
    columns: z
      .array(
        z.object({
          key: z.string().min(1).max(128),
          label: z.string().max(160).optional(),
          type: z
            .enum(["string", "number", "boolean", "datetime", "currency"])
            .optional()
        })
      )
      .min(1),
    rows_preview: z.array(z.record(z.string(), jsonValueSchema)).default([]),
    data_ref: artifactRefSchema.optional()
  })
  .transform((value) => ({
    kind: value.kind,
    title: value.title,
    columns: value.columns,
    rowsPreview: value.rows_preview,
    dataRef: value.data_ref
  }));

const chartViewSchema = z
  .object({
    kind: z.literal("chart"),
    title: titleSchema,
    spec_kind: z.literal("vega_lite"),
    spec: z.record(z.string(), jsonValueSchema),
    data_ref: artifactRefSchema.optional()
  })
  .transform((value) => ({
    kind: value.kind,
    title: value.title,
    specKind: value.spec_kind,
    spec: value.spec,
    dataRef: value.data_ref
  }));

const mapViewSchema = z
  .object({
    kind: z.literal("map"),
    title: titleSchema,
    format: z.literal("geojson"),
    data_ref: artifactRefSchema,
    data_preview: z.record(z.string(), jsonValueSchema).optional(),
    style_ref: z.string().max(160).optional()
  })
  .transform((value) => ({
    kind: value.kind,
    title: value.title,
    format: value.format,
    dataRef: value.data_ref,
    dataPreview: value.data_preview,
    styleRef: value.style_ref
  }));

const jsonViewSchema = z
  .object({
    kind: z.literal("json"),
    title: titleSchema,
    value_preview: jsonValueSchema,
    data_ref: artifactRefSchema.optional()
  })
  .transform((value) => ({
    kind: value.kind,
    title: value.title,
    valuePreview: value.value_preview,
    dataRef: value.data_ref
  }));

const fileDiffViewSchema = z
  .object({
    kind: z.literal("file_diff"),
    title: titleSchema,
    files: z.array(jsonValueSchema)
  })
  .transform((value) => ({
    kind: value.kind,
    title: value.title,
    files: value.files
  }));

const markdownViewSchema = z.object({
  kind: z.literal("markdown"),
  title: titleSchema,
  text: z.string().max(4000)
});

const artifactViewSchema = z
  .object({
    kind: z.literal("artifact"),
    title: titleSchema,
    artifact_ref: artifactRefSchema
  })
  .transform((value) => ({
    kind: value.kind,
    title: value.title,
    artifactRef: value.artifact_ref
  }));

const toolResultViewSchema = z.union([
  tableViewSchema,
  chartViewSchema,
  mapViewSchema,
  jsonViewSchema,
  fileDiffViewSchema,
  markdownViewSchema,
  artifactViewSchema
]);

export function parseToolResultViews(value: JsonValue | undefined): ToolResultView[] {
  if (!Array.isArray(value)) {
    return [];
  }

  return value.flatMap((item) => {
    const parsed = toolResultViewSchema.safeParse(item);
    return parsed.success ? [parsed.data as ToolResultView] : [];
  });
}
