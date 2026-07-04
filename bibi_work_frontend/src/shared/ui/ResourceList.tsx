import type { ReactNode } from "react";

export function ResourceList({
  className = "",
  children
}: {
  className?: string;
  children: ReactNode;
}) {
  return <div className={`resource-list ${className}`}>{children}</div>;
}
