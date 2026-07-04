import { forwardRef, type TextareaHTMLAttributes } from "react";

export const TextArea = forwardRef<HTMLTextAreaElement, TextareaHTMLAttributes<HTMLTextAreaElement>>(
  function TextArea({ className = "", ...props }, ref) {
    return <textarea ref={ref} className={`text-area ${className}`} {...props} />;
  }
);
