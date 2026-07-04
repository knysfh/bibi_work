import type { MouseEvent, ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import rehypeKatex from "rehype-katex";
import rehypeSanitize, { defaultSchema, type Options as SanitizeSchema } from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import "katex/dist/katex.min.css";
import { openControlledExternalUrl } from "../../../shared/tauri/external-open";
import { CodeBlock } from "./CodeBlock";
import { DrawioArtifact } from "./DrawioArtifact";
import { MermaidBlock } from "./MermaidBlock";
import { SafeHtmlFrame } from "./SafeHtmlFrame";
import { SafeSvgPreview } from "./SafeSvgPreview";

interface MessageContentRendererProps {
  content: string;
}

const markdownSanitizeSchema: SanitizeSchema = {
  ...defaultSchema,
  attributes: {
    ...defaultSchema.attributes,
    code: [...(defaultSchema.attributes?.code ?? []), ["className", /^language-[\w.-]+$/]],
    pre: [...(defaultSchema.attributes?.pre ?? []), ["className", /^language-[\w.-]+$/]]
  },
  protocols: {
    ...defaultSchema.protocols,
    href: ["http", "https", "mailto"]
  },
  strip: [...(defaultSchema.strip ?? []), "script", "style"]
};

const components: Components = {
  a({ href, children }) {
    return <SafeMarkdownLink href={href}>{children}</SafeMarkdownLink>;
  },
  code({ className, children, ...props }) {
    const rawCode = String(children);
    const code = rawCode.replace(/\n$/, "");
    const language = normalizeCodeLanguage(className);
    const isBlock = Boolean(language) || rawCode.endsWith("\n");

    if (!isBlock) {
      return (
        <code className={className} {...props}>
          {children}
        </code>
      );
    }

    if (language === "mermaid") {
      return <MermaidBlock chart={code} />;
    }

    if (language === "html") {
      return <SafeHtmlFrame html={code} />;
    }

    if (language === "svg") {
      return <SafeSvgPreview svg={code} />;
    }

    if (
      language === "drawio" ||
      language === ".drawio" ||
      language === "mxfile" ||
      language === ".mxfile"
    ) {
      return <DrawioArtifact source={code} />;
    }

    return <CodeBlock code={code} language={language} />;
  },
  img({ src, alt }) {
    if (!src || !isSafeInlineImage(src)) {
      return null;
    }
    return <img src={src} alt={alt ?? ""} loading="lazy" />;
  },
  pre({ children }) {
    return <>{children}</>;
  }
};

export function MessageContentRenderer({ content }: MessageContentRendererProps) {
  return (
    <div className="message-markdown">
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkMath]}
        rehypePlugins={[[rehypeSanitize, markdownSanitizeSchema], rehypeKatex]}
        components={components}
        skipHtml
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}

function normalizeCodeLanguage(className?: string): string | undefined {
  const match = /(?:^|\s)language-([\w.-]+)/.exec(className ?? "");
  return match?.[1]?.trim().toLowerCase();
}

function isSafeInlineImage(src: string): boolean {
  return /^data:image\/(?:png|jpe?g|gif|webp);base64,[a-z0-9+/=]+$/i.test(src.trim());
}

function SafeMarkdownLink({ href, children }: { href?: string; children: ReactNode }) {
  const link = classifyHref(href);

  if (link.kind === "anchor") {
    return <a href={link.href}>{children}</a>;
  }

  if (link.kind === "external") {
    return (
      <a href={link.href} onClick={(event) => void openExternalLink(event, link.href)}>
        {children}
      </a>
    );
  }

  return <span className="message-disabled-link">{children}</span>;
}

type ClassifiedHref =
  | { kind: "anchor"; href: string }
  | { kind: "external"; href: string }
  | { kind: "blocked" };

function classifyHref(href?: string): ClassifiedHref {
  const trimmed = href?.trim();
  if (!trimmed) {
    return { kind: "blocked" };
  }
  if (trimmed.startsWith("#")) {
    return { kind: "anchor", href: trimmed };
  }

  try {
    const parsed = new URL(trimmed);
    if (
      parsed.protocol === "http:" ||
      parsed.protocol === "https:" ||
      parsed.protocol === "mailto:"
    ) {
      return { kind: "external", href: parsed.toString() };
    }
  } catch {
    return { kind: "blocked" };
  }

  return { kind: "blocked" };
}

async function openExternalLink(event: MouseEvent<HTMLAnchorElement>, href: string) {
  event.preventDefault();
  await openControlledExternalUrl(href);
}
