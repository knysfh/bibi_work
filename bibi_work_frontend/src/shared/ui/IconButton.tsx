import type { ButtonHTMLAttributes, ReactNode } from "react";

interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  label: string;
  icon: ReactNode;
}

export function IconButton({ label, icon, className = "", ...props }: IconButtonProps) {
  return (
    <button className={`icon-button ${className}`} aria-label={label} title={label} {...props}>
      {icon}
    </button>
  );
}
