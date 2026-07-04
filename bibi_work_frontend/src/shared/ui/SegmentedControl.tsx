import type { ReactNode } from "react";
import { SelectableGroup } from "./SelectableGroup";

export interface SegmentedControlItem<T extends string> {
  id: T;
  label: ReactNode;
}

export function SegmentedControl<T extends string>({
  label,
  items,
  active,
  onChange,
  className = ""
}: {
  label: string;
  items: Array<SegmentedControlItem<T>>;
  active: T;
  onChange: (id: T) => void;
  className?: string;
}) {
  return (
    <SelectableGroup
      label={label}
      className={`segmented-control ${className}`}
      items={items}
      active={active}
      onChange={onChange}
    />
  );
}
