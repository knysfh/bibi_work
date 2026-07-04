import { SelectableGroup } from "./SelectableGroup";

export interface TabItem<T extends string> {
  id: T;
  label: string;
}

export function Tabs<T extends string>({
  items,
  active,
  onChange
}: {
  items: TabItem<T>[];
  active: T;
  onChange: (tab: T) => void;
}) {
  return <SelectableGroup className="tabs" items={items} active={active} onChange={onChange} />;
}
