import { useEffect, useState } from "react";
import DOMPurify from "dompurify";
import { sanitizeSvg } from "./SafeSvgPreview";

interface MermaidBlockProps {
  chart: string;
}

let mermaidRenderCounter = 0;

export function MermaidBlock({ chart }: MermaidBlockProps) {
  const [state, setState] = useState<
    { status: "loading" } | { status: "ready"; svg: string } | { status: "failed"; error: string }
  >({ status: "loading" });

  useEffect(() => {
    let cancelled = false;
    setState({ status: "loading" });

    async function renderMermaid() {
      try {
        const mermaidModule = await import("mermaid");
        const mermaid = mermaidModule.default;
        mermaid.initialize({
          startOnLoad: false,
          securityLevel: "strict",
          theme: "default"
        });
        const renderId = `bibi-mermaid-${++mermaidRenderCounter}`;
        const rendered = await mermaid.render(renderId, chart);
        const svg = DOMPurify.sanitize(sanitizeSvg(rendered.svg), {
          USE_PROFILES: { svg: true, svgFilters: true }
        });
        if (!cancelled) {
          setState({ status: "ready", svg });
        }
      } catch (error) {
        if (!cancelled) {
          setState({
            status: "failed",
            error: error instanceof Error ? error.message : "Unable to render diagram"
          });
        }
      }
    }

    void renderMermaid();

    return () => {
      cancelled = true;
    };
  }, [chart]);

  if (state.status === "loading") {
    return <div className="message-artifact-placeholder">Rendering diagram...</div>;
  }

  if (state.status === "failed") {
    return (
      <pre className="message-artifact-error">
        <code>{state.error}</code>
      </pre>
    );
  }

  return (
    <div
      className="message-mermaid-preview"
      data-testid="mermaid-block"
      dangerouslySetInnerHTML={{ __html: state.svg }}
    />
  );
}
