interface BadgeProps {
  tone?: "neutral" | "info" | "success" | "warning" | "danger";
  children: string;
}

export function Badge({ tone = "neutral", children }: BadgeProps) {
  return <span className={`badge badge-${tone}`}>{children}</span>;
}
