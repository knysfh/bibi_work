import { useMemo } from "react";
import DOMPurify from "dompurify";

interface SafeHtmlFrameProps {
  html: string;
}

export function SafeHtmlFrame({ html }: SafeHtmlFrameProps) {
  const srcDoc = useMemo(() => {
    const sanitized = DOMPurify.sanitize(html, {
      USE_PROFILES: { html: true },
      FORBID_TAGS: [
        "script",
        "style",
        "iframe",
        "object",
        "embed",
        "link",
        "meta",
        "base",
        "form",
        "input",
        "button",
        "textarea",
        "select",
        "img",
        "audio",
        "video",
        "source",
        "track"
      ],
      FORBID_ATTR: ["style", "src", "srcset", "srcdoc"]
    });
    return `<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <style>
      :root { color-scheme: light dark; }
      body {
        margin: 0;
        padding: 12px;
        color: #1f2937;
        background: #ffffff;
        font: 14px/1.5 system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      }
      pre, code {
        font-family: "SFMono-Regular", Consolas, "Liberation Mono", monospace;
      }
      table {
        border-collapse: collapse;
      }
      th, td {
        border: 1px solid #d1d5db;
        padding: 4px 6px;
      }
    </style>
  </head>
  <body>${sanitized}</body>
</html>`;
  }, [html]);

  return (
    <iframe
      className="message-html-frame"
      data-testid="safe-html-frame"
      sandbox=""
      referrerPolicy="no-referrer"
      srcDoc={srcDoc}
      title="HTML preview"
    />
  );
}
