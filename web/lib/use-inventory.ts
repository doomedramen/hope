"use client";
import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "@/lib/api";
import type { Bootstrap, Host } from "@/lib/protocol";

/** Owns snapshot recovery and live inventory updates for one mounted console. */
export function useInventory() {
  const [data, setData] = useState<Bootstrap | null>(null);
  const [error, setError] = useState("");
  const [connected, setConnected] = useState(false);
  const mounted = useRef(false);
  const pending = useRef<Promise<void> | null>(null);
  const refreshAgain = useRef(false);
  const request = useRef<AbortController | null>(null);
  const knownHosts = useRef(new Set<string>());
  const sequence = useRef(0);
  const hostVersions = useRef(new Map<string, number>());
  const refresh = useCallback((): Promise<void> => {
    refreshAgain.current = true;
    if (pending.current) return pending.current;
    const load = async () => {
      do {
        refreshAgain.current = false;
        if (!mounted.current) return;
        const controller = new AbortController();
        request.current = controller;
        const startedAt = sequence.current;
        try {
          const latest = await api<Bootstrap>("/bootstrap", {
            signal: controller.signal,
          });
          if (!mounted.current || controller.signal.aborted) return;
          knownHosts.current = new Set(latest.hosts.map((host) => host.id));
          for (const id of hostVersions.current.keys())
            if (!knownHosts.current.has(id)) hostVersions.current.delete(id);
          setData((previous) => {
            if (!previous) return latest;
            const live = new Map(previous.hosts.map((host) => [host.id, host]));
            return {
              ...latest,
              hosts: latest.hosts.map((host) =>
                (hostVersions.current.get(host.id) ?? 0) > startedAt
                  ? (live.get(host.id) ?? host)
                  : host,
              ),
              map:
                previous.map.revision > latest.map.revision
                  ? previous.map
                  : latest.map,
            };
          });
          setError("");
        } catch (cause) {
          if (mounted.current && !controller.signal.aborted)
            setError((cause as Error).message);
        } finally {
          request.current = null;
        }
      } while (refreshAgain.current && mounted.current);
    };
    pending.current = load().finally(() => {
      pending.current = null;
    });
    return pending.current;
  }, []);

  useEffect(() => {
    mounted.current = true;
    const initial = setTimeout(() => void refresh(), 0);
    let timer: ReturnType<typeof setTimeout> | undefined;
    const schedule = () => {
      if (!timer)
        timer = setTimeout(() => {
          timer = undefined;
          void refresh();
        }, 500);
    };
    const source = new EventSource("/api/v1/events");
    source.onopen = () => {
      setConnected(true);
      schedule();
    };
    source.onerror = () => setConnected(false);
    source.addEventListener("refresh", schedule);
    source.addEventListener("resync", schedule);
    source.addEventListener("host", (event) => {
      let host: Host;
      try {
        host = JSON.parse((event as MessageEvent<string>).data) as Host;
      } catch {
        return schedule();
      }
      if (!knownHosts.current.has(host.id)) return schedule();
      hostVersions.current.set(host.id, ++sequence.current);
      setData((previous) =>
        previous
          ? {
              ...previous,
              hosts: previous.hosts.map((existing) =>
                existing.id === host.id ? host : existing,
              ),
            }
          : previous,
      );
    });
    const fallback = setInterval(() => void refresh(), 15000);
    return () => {
      mounted.current = false;
      request.current?.abort();
      clearTimeout(initial);
      clearTimeout(timer);
      clearInterval(fallback);
      source.close();
    };
  }, [refresh]);
  return { data, setData, error, setError, connected, refresh };
}
