import type { ReactNode } from "react";

export interface SelectableGroupItem<T extends string> {
  id: T;
  label: ReactNode;
}

export function SelectableGroup<T extends string>({
  label,
  items,
  active,
  onChange,
  className = ""
}: {
  label?: string;
  items: Array<SelectableGroupItem<T>>;
  active: T;
  onChange: (id: T) => void;
  className?: string;
}) {
  return (
    <div className={`selectable-group ${className}`} role="tablist" aria-label={label}>
      {items.map((item) => (
        <button
          key={item.id}
          type="button"
          role="tab"
          aria-selected={item.id === active}
          className={item.id === active ? "active" : ""}
          onClick={() => onChange(item.id)}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}
