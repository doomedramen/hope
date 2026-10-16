import { usageTone, type Tone } from "@/lib/fleet";

/** A thin usage bar whose fill carries severity; an unknown value shows an empty track. */
export function Meter({
  value,
  tone,
  label,
  thin,
  className,
}: {
  value: number | null | undefined;
  tone?: Tone;
  label?: string;
  thin?: boolean;
  className?: string;
}) {
  const known = value != null && Number.isFinite(value);
  const width = known ? Math.min(100, Math.max(0, value)) : 0;
  return (
    <div
      className={`meter ${thin ? "thin" : ""} ${className ?? ""}`}
      data-tone={known ? (tone ?? usageTone(value)) : "unknown"}
      role={label ? "meter" : undefined}
      aria-label={label}
      aria-valuemin={label ? 0 : undefined}
      aria-valuemax={label ? 100 : undefined}
      aria-valuenow={label && known ? Math.round(value) : undefined}
      aria-valuetext={label && !known ? "Unavailable" : undefined}
    >
      <span style={{ width: `${width}%` }} />
    </div>
  );
}
