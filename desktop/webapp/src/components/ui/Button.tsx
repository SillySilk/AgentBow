import type { ButtonHTMLAttributes } from "react";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: "forge" | "ember" | "ghost" | "danger";
  size?: "sm" | "md" | "lg";
  block?: boolean;
}

export default function Button({ variant = "ghost", size = "md", block, className, children, ...rest }: ButtonProps) {
  const cls = ["btn", `btn-${variant}`, `btn-${size}`, block ? "btn-block" : "", className].filter(Boolean).join(" ");
  return (
    <button className={cls} {...rest}>
      {children}
    </button>
  );
}
