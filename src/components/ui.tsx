import { useEffect, type ReactNode } from "react";
import { X } from "lucide-react";

export function cx(...classes: (string | false | null | undefined)[]) {
  return classes.filter(Boolean).join(" ");
}

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";

const VARIANTS: Record<ButtonVariant, string> = {
  primary: "bg-accent text-white hover:bg-accent-strong disabled:bg-accent/40 disabled:text-white/60",
  secondary: "bg-raised text-fg border border-line hover:bg-hover hover:border-line-strong disabled:opacity-50",
  ghost: "text-muted hover:text-fg hover:bg-hover disabled:opacity-40",
  danger: "bg-danger/15 text-danger hover:bg-danger/25 disabled:opacity-50",
};

export function Button({
  variant = "secondary",
  size = "md",
  className,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { variant?: ButtonVariant; size?: "sm" | "md" | "lg" }) {
  const sizes = { sm: "h-7 px-2.5 text-xs gap-1.5", md: "h-8 px-3 gap-2", lg: "h-10 px-4 text-sm gap-2" };
  return (
    <button
      type="button"
      className={cx(
        "inline-flex items-center justify-center rounded-md font-medium transition-colors whitespace-nowrap disabled:pointer-events-none",
        sizes[size],
        VARIANTS[variant],
        className,
      )}
      {...props}
    />
  );
}

export function IconButton({
  label,
  className,
  active,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { label: string; active?: boolean }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      className={cx(
        "inline-flex h-8 w-8 items-center justify-center rounded-md transition-colors disabled:opacity-40",
        active ? "bg-accent-soft text-accent" : "text-muted hover:text-fg hover:bg-hover",
        className,
      )}
      {...props}
    />
  );
}

export function Toggle({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  label?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cx(
        "relative inline-flex h-[18px] w-8 shrink-0 items-center rounded-full transition-colors disabled:opacity-40",
        checked ? "bg-accent" : "bg-line-strong",
      )}
    >
      <span
        className={cx(
          "inline-block h-3.5 w-3.5 rounded-full bg-white shadow transition-transform",
          checked ? "translate-x-[16px]" : "translate-x-[2px]",
        )}
      />
    </button>
  );
}

export function Checkbox({
  checked,
  indeterminate,
  onChange,
  color,
  label,
}: {
  checked: boolean;
  indeterminate?: boolean;
  onChange: (v: boolean) => void;
  color?: string;
  label?: string;
}) {
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={indeterminate ? "mixed" : checked}
      aria-label={label}
      title={label}
      onClick={(e) => {
        e.stopPropagation();
        onChange(!checked);
      }}
      className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-[4px] border transition-colors"
      style={{
        borderColor: checked || indeterminate ? color ?? "var(--color-accent)" : "var(--color-line-strong)",
        background: checked ? color ?? "var(--color-accent)" : "transparent",
      }}
    >
      {checked && !indeterminate && (
        <svg viewBox="0 0 12 12" className="h-3 w-3 text-black/80">
          <path d="M2.5 6.2 5 8.5l4.5-5" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      )}
      {indeterminate && <span className="h-0.5 w-2 rounded" style={{ background: color ?? "var(--color-accent)" }} />}
    </button>
  );
}

export function Slider({
  value,
  min,
  max,
  step,
  onChange,
  format,
  disabled,
  label,
}: {
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (v: number) => void;
  format: (v: number) => string;
  disabled?: boolean;
  label: string;
}) {
  return (
    <label className={cx("flex flex-col gap-1", disabled && "opacity-40")}>
      <span className="flex items-center justify-between text-xs text-muted">
        <span>{label}</span>
        <span className="font-mono text-fg/80">{format(value)}</span>
      </span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(Number(e.target.value))}
        className="h-1 w-full"
      />
    </label>
  );
}

export function TextInput({ className, ...props }: React.InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={cx(
        "h-8 w-full rounded-md border border-line bg-bg px-2.5 text-fg placeholder:text-faint focus:border-accent focus:outline-none disabled:opacity-50",
        className,
      )}
      {...props}
    />
  );
}

export function Select({
  value,
  onChange,
  options,
  className,
  disabled,
}: {
  value: string;
  onChange: (v: string) => void;
  options: { value: string; label: string }[];
  className?: string;
  disabled?: boolean;
}) {
  return (
    <select
      value={value}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value)}
      className={cx(
        "h-8 w-full rounded-md border border-line bg-bg px-2 text-fg focus:border-accent focus:outline-none disabled:opacity-50",
        className,
      )}
    >
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

export function ProgressBar({ value, className }: { value: number; className?: string }) {
  return (
    <div className={cx("h-1.5 w-full overflow-hidden rounded-full bg-line", className)}>
      <div
        className="h-full rounded-full bg-accent transition-[width] duration-200"
        style={{ width: `${Math.round(Math.min(1, Math.max(0, value)) * 100)}%` }}
      />
    </div>
  );
}

export function Modal({
  title,
  onClose,
  children,
  footer,
  width = 560,
}: {
  title: ReactNode;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  width?: number;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6" onMouseDown={onClose}>
      <div
        role="dialog"
        aria-modal="true"
        className="flex max-h-full w-full flex-col overflow-hidden rounded-xl border border-line bg-panel shadow-2xl"
        style={{ maxWidth: width }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between border-b border-line px-5 py-3">
          <h2 className="text-sm font-semibold">{title}</h2>
          <IconButton label="Close" onClick={onClose}>
            <X size={16} />
          </IconButton>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto">{children}</div>
        {footer && <div className="flex justify-end gap-2 border-t border-line px-5 py-3">{footer}</div>}
      </div>
    </div>
  );
}
