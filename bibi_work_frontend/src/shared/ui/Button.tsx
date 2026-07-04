import type { ButtonHTMLAttributes, ReactNode } from "react";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: "primary" | "secondary" | "ghost" | "danger";
  size?: "sm" | "md" | "icon";
  icon?: ReactNode;
}

export function Button({
  variant = "secondary",
  size = "md",
  icon,
  children,
  className = "",
  ...props
}: ButtonProps) {
  return (
    <button className={`button button-${variant} button-${size} ${className}`} {...props}>
      {icon}
      {children ? <span>{children}</span> : null}
    </button>
  );
}

export function DangerActionButton(props: Omit<ButtonProps, "variant">) {
  return <Button {...props} variant="danger" />;
}
