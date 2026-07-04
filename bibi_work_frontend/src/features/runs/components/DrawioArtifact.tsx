import { useMemo } from "react";
import { SafeSvgPreview } from "./SafeSvgPreview";

interface DrawioArtifactProps {
  source: string;
}

const DATA_IMAGE_PATTERN = /data:image\/(?:png|jpe?g|webp);base64,[a-z0-9+/=]+/i;

export function DrawioArtifact({ source }: DrawioArtifactProps) {
  const artifact = useMemo(() => parseDrawioArtifact(source), [source]);

  if (artifact.kind === "svg") {
    return <SafeSvgPreview svg={artifact.svg} />;
  }

  if (artifact.kind === "image") {
    return (
      <div className="message-drawio-artifact" data-testid="drawio-artifact">
        <img src={artifact.src} alt="drawio artifact" />
      </div>
    );
  }

  return (
    <iframe
      className="message-drawio-frame"
      data-testid="drawio-artifact"
      sandbox=""
      referrerPolicy="no-referrer"
      srcDoc={artifact.srcDoc}
      title="drawio artifact"
    />
  );
}

type DrawioArtifactResult =
  | { kind: "svg"; svg: string }
  | { kind: "image"; src: string }
  | { kind: "xml"; srcDoc: string };

function parseDrawioArtifact(source: string): DrawioArtifactResult {
  const svg = extractSvg(source);
  if (svg) {
    return { kind: "svg", svg };
  }

  const image = source.match(DATA_IMAGE_PATTERN)?.[0];
  if (image) {
    return { kind: "image", src: image };
  }

  return { kind: "xml", srcDoc: createXmlPreview(source) };
}

function extractSvg(source: string): string | null {
  const trimmed = source.trim();
  if (/^<svg[\s>]/i.test(trimmed)) {
    return trimmed;
  }
  return source.match(/<svg[\s\S]*<\/svg>/i)?.[0] ?? null;
}

function createXmlPreview(source: string): string {
  return `<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <style>
      body {
        margin: 0;
        padding: 12px;
        color: #1f2937;
        background: #ffffff;
        font: 12px/1.5 "SFMono-Regular", Consolas, "Liberation Mono", monospace;
      }
      pre {
        margin: 0;
        white-space: pre-wrap;
        overflow-wrap: anywhere;
      }
    </style>
  </head>
  <body><pre>${escapeHtml(source)}</pre></body>
</html>`;
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}
