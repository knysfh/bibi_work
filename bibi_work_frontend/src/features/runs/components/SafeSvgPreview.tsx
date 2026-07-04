import { useMemo } from "react";
import DOMPurify from "dompurify";

interface SafeSvgPreviewProps {
  svg: string;
}

export function SafeSvgPreview({ svg }: SafeSvgPreviewProps) {
  const sanitizedSvg = useMemo(() => sanitizeSvg(svg), [svg]);

  return (
    <div
      className="message-svg-preview"
      data-testid="safe-svg-preview"
      dangerouslySetInnerHTML={{ __html: sanitizedSvg }}
    />
  );
}

export function sanitizeSvg(svg: string): string {
  return DOMPurify.sanitize(svg, {
    USE_PROFILES: { svg: true, svgFilters: true },
    FORBID_TAGS: ["script", "foreignObject"],
    FORBID_ATTR: ["href", "xlink:href", "src", "srcset", "style"]
  });
}
