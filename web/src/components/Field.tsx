import { type ReactNode, type ChangeEvent } from "react";

export default function Field({
  label,
  hint,
  error,
  required,
  children,
}: {
  label: string;
  hint?: string;
  error?: string;
  required?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1">
      <label className="text-xs text-fg-3 font-medium">
        {label}
        {required && <span className="text-rose-400 ml-0.5">*</span>}
      </label>
      {children}
      {hint && !error && (
        <p className="text-[11px] text-fg-4">{hint}</p>
      )}
      {error && (
        <p className="text-[11px] text-rose-400">{error}</p>
      )}
    </div>
  );
}

const inputBase =
  "w-full px-3 h-9 rounded-md bg-bg-3 border border-border text-[13px] text-fg outline-none focus-ring placeholder:text-fg-4 disabled:opacity-50";

export function TextInput({
  value,
  onChange,
  placeholder,
  type = "text",
  disabled,
  className = "",
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  type?: string;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <input
      type={type}
      value={value}
      onChange={(e: ChangeEvent<HTMLInputElement>) => onChange(e.target.value)}
      placeholder={placeholder}
      disabled={disabled}
      className={`${inputBase} ${className}`}
    />
  );
}

export function TextArea({
  value,
  onChange,
  placeholder,
  rows = 4,
  disabled,
  className = "",
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  rows?: number;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <textarea
      value={value}
      onChange={(e: ChangeEvent<HTMLTextAreaElement>) => onChange(e.target.value)}
      placeholder={placeholder}
      rows={rows}
      disabled={disabled}
      className={`w-full px-3 py-2 rounded-md bg-bg-3 border border-border text-[13px] text-fg outline-none focus-ring placeholder:text-fg-4 resize-y disabled:opacity-50 ${className}`}
    />
  );
}

export function Select({
  value,
  onChange,
  placeholder,
  disabled,
  className = "",
  children,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <select
      value={value}
      onChange={(e: ChangeEvent<HTMLSelectElement>) => onChange(e.target.value)}
      disabled={disabled}
      className={`w-full px-3 h-9 rounded-md bg-bg-3 border border-border text-[13px] text-fg outline-none focus-ring disabled:opacity-50 ${className}`}
    >
      {placeholder && (
        <option value="" disabled>
          {placeholder}
        </option>
      )}
      {children}
    </select>
  );
}
