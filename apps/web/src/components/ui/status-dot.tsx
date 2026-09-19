import { cn } from "cn";

type StatusTone = "healthy" | "attention" | "critical" | "info" | "neutral";

const toneClasses: Record<StatusTone, string> = {
  healthy: "bg-status-healthy-fg/85",
  attention: "bg-status-attention-fg/80",
  critical: "bg-status-critical-fg/80",
  info: "bg-status-info-fg/80",
  neutral: "bg-status-neutral-fg/75",
};

function StatusDot({
  className,
  label,
  tone,
}: {
  className?: string;
  label?: string;
  tone: StatusTone;
}) {
  return (
    <span
      aria-hidden={label ? undefined : true}
      aria-label={label}
      className={cn(
        "size-2 shrink-0 rounded-full",
        toneClasses[tone],
        className,
      )}
      role={label ? "img" : undefined}
    />
  );
}

export { StatusDot, type StatusTone };
