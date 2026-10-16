"use client";
import { age } from "@/lib/api";
import { useNow } from "@/lib/clock";

/** A relative time that keeps counting while it is on screen. */
export function Age({
  value,
  lower = false,
}: {
  value: number | null | undefined;
  lower?: boolean;
}) {
  const text = age(value, useNow());
  return <>{lower ? text.toLowerCase() : text}</>;
}
