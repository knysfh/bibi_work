export function KeyValue({ label, value }: { label: string; value?: string | null }) {
  return (
    <div className="key-value">
      <dt>{label}</dt>
      <dd>{value || "-"}</dd>
    </div>
  );
}
