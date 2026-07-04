import type { ReactNode } from "react";

export function Panel({
  title,
  subtitle,
  actions,
  className = "",
  children
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  actions?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  return (
    <section className={`page-panel ${className}`}>
      <header className="panel-header">
        <div>
          <strong>{title}</strong>
          {subtitle ? <span>{subtitle}</span> : null}
        </div>
        {actions ? <div className="panel-header-actions">{actions}</div> : null}
      </header>
      {children}
    </section>
  );
}
