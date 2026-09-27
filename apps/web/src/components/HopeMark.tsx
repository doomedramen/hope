/** The Hope mark is decorative wherever the adjacent text names the app. */
export function HopeMark({ className = "size-8" }: { className?: string }) {
  return (
    <img
      alt=""
      aria-hidden="true"
      className={`shrink-0 ${className}`}
      height={64}
      src="/hope-logo.svg"
      width={64}
    />
  );
}
