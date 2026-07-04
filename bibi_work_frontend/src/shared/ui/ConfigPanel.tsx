import { X } from "lucide-react";
import { useEffect, useId, type ReactNode } from "react";

interface ConfigPanelProps {
  title: string;
  subtitle?: string;
  closeLabel?: string;
  onClose: () => void;
  children: ReactNode;
}

export function ConfigPanel({
  title,
  subtitle,
  closeLabel = "Close",
  onClose,
  children
}: ConfigPanelProps) {
  const titleId = useId();

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onClose();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  return (
    <div className="config-panel-overlay" role="presentation" onMouseDown={onClose}>
      <section
        className="config-panel"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header className="config-panel-header">
          <div>
            <strong id={titleId}>{title}</strong>
            {subtitle ? <span>{subtitle}</span> : null}
          </div>
          <button className="icon-button" type="button" aria-label={closeLabel} onClick={onClose}>
            <X size={16} />
          </button>
        </header>
        <div className="config-panel-body">{children}</div>
      </section>
    </div>
  );
}
