import type { ReactNode } from "react";

export interface TagProps {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
}

export default function Tag({ active, onClick, children }: TagProps) {
  return (
    <button type="button" className={`tag${active ? " on" : ""}`} onClick={onClick}>
      {children}
    </button>
  );
}
