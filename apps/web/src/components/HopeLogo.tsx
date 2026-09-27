/** Outlined lettering keeps the Hope wordmark independent of installed fonts. */
export function HopeLogo({ className = "h-9" }: { className?: string }) {
  return (
    <img
      alt="Hope"
      className={`w-auto shrink-0 dark:brightness-0 dark:invert ${className}`}
      height={100}
      src="/hope-wordmark.svg"
      width={262}
    />
  );
}
