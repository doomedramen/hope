import { useSyncExternalStore } from "react";

// One timer drives every live clock on the page; it runs only while something listens.
const listeners = new Set<() => void>();
let current = Date.now() / 1000;
let timer: ReturnType<typeof setInterval> | undefined;

function subscribe(listener: () => void) {
  listeners.add(listener);
  if (!timer) {
    current = Date.now() / 1000;
    timer = setInterval(() => {
      current = Date.now() / 1000;
      for (const notify of listeners) notify();
    }, 1000);
  }
  return () => {
    listeners.delete(listener);
    if (!listeners.size) {
      clearInterval(timer);
      timer = undefined;
    }
  };
}

/** Shared wall clock; a larger resolution limits expensive callers' re-renders. */
export function useNow(resolutionSeconds = 1): number {
  return useSyncExternalStore(
    subscribe,
    () => Math.floor(current / resolutionSeconds) * resolutionSeconds,
    () => Math.floor(current / resolutionSeconds) * resolutionSeconds,
  );
}
