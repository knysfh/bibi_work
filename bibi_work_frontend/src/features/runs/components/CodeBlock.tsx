import { useMemo } from "react";
import DOMPurify from "dompurify";
import hljs from "highlight.js";
import "highlight.js/styles/github.css";

interface CodeBlockProps {
  code: string;
  language?: string;
}

export function CodeBlock({ code, language }: CodeBlockProps) {
  const highlighted = useMemo(() => {
    const normalizedLanguage = language?.trim().toLowerCase();
    const html =
      normalizedLanguage && hljs.getLanguage(normalizedLanguage)
        ? hljs.highlight(code, { language: normalizedLanguage, ignoreIllegals: true }).value
        : escapeHtml(code);
    return DOMPurify.sanitize(html, {
      ALLOWED_TAGS: ["span"],
      ALLOWED_ATTR: ["class"]
    });
  }, [code, language]);

  return (
    <pre className="message-code-block">
      <code
        className={language ? `language-${language}` : undefined}
        dangerouslySetInnerHTML={{ __html: highlighted }}
      />
    </pre>
  );
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}
