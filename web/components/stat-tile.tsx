import type { ReactNode } from "react";
import type { Tone } from "@/lib/fleet";

/** A headline figure: label, value, hint and optional trend. */
export function StatTile({
  label,
  icon,
  value,
  unit,
  hint,
  tone,
  trend,
  onClick,
  testId,
}: {
  label: string;
  icon?: ReactNode;
  value: ReactNode;
  unit?: ReactNode;
  hint?: ReactNode;
  tone?: Tone;
  trend?: ReactNode;
  onClick?: () => void;
  testId?: string;
}) {
  const body = (
    <>
      <header>
        <span>{label}</span>
        {icon}
      </header>
      <div className="value">
        {value}
        {unit && <small>{unit}</small>}
      </div>
      {trend && <div className="spark">{trend}</div>}
      {hint !== undefined && <div className="hint">{hint}</div>}
    </>
  );
  if (onClick)
    return (
      <button
        type="button"
        className="stat-tile clickable"
        data-tone={tone}
        data-testid={testId}
        onClick={onClick}
      >
        {body}
      </button>
    );
  return (
    <div className="stat-tile" data-tone={tone} data-testid={testId}>
      {body}
    </div>
  );
}
