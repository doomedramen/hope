import {
  Box,
  Cast,
  Cctv,
  CircleDot,
  CircleHelp,
  Gamepad2,
  HardDrive,
  Laptop,
  Layers,
  Lightbulb,
  Network,
  Printer,
  Router,
  Server,
  Smartphone,
  Speaker,
  Tablet,
  Tv,
  WashingMachine,
  Wifi,
  type LucideIcon,
} from "lucide-react";
import type { Host } from "@/lib/protocol";
import { findSelfHostedIcon, findUniFiIcon } from "@/lib/icon-matching";

/** Device kinds the server can guess, in the order the console offers them (mirrors the server). */
export const DEVICE_KINDS: { kind: string; label: string; icon: LucideIcon }[] =
  [
    { kind: "router", label: "Router", icon: Router },
    { kind: "switch", label: "Switch", icon: Network },
    { kind: "access_point", label: "Access point", icon: Wifi },
    { kind: "hypervisor", label: "Hypervisor", icon: Layers },
    { kind: "nas", label: "NAS", icon: HardDrive },
    { kind: "server", label: "Server", icon: Server },
    { kind: "vm", label: "Virtual machine", icon: Box },
    { kind: "computer", label: "Computer", icon: Laptop },
    { kind: "phone", label: "Phone", icon: Smartphone },
    { kind: "tablet", label: "Tablet", icon: Tablet },
    { kind: "tv", label: "TV", icon: Tv },
    { kind: "streamer", label: "Media streamer", icon: Cast },
    { kind: "speaker", label: "Smart speaker", icon: Speaker },
    { kind: "printer", label: "Printer", icon: Printer },
    { kind: "camera", label: "Camera", icon: Cctv },
    { kind: "vacuum", label: "Robot vacuum", icon: CircleDot },
    { kind: "appliance", label: "Home appliance", icon: WashingMachine },
    { kind: "game_console", label: "Game console", icon: Gamepad2 },
    { kind: "smart_home", label: "Smart home device", icon: Lightbulb },
    { kind: "unknown", label: "Unknown device", icon: CircleHelp },
  ];

const BY_KIND = new Map(DEVICE_KINDS.map((k) => [k.kind, k]));

/** The kind to draw for a host: the server's guess, else what an agent says, else unknown. */
export function deviceKind(host: Host): string {
  const guessed = host.device?.kind;
  if (guessed && guessed !== "unknown") return guessed;
  const os = host.inventory?.os.toLowerCase() ?? "";
  if (os.includes("mac") || os.includes("windows")) return "computer";
  if (host.agent_id) return "server";
  return "unknown";
}

export function deviceIcon(host: Host): LucideIcon {
  return BY_KIND.get(deviceKind(host))?.icon ?? CircleHelp;
}

export function hostDisplayName(host: Host): string {
  return host.name_override?.trim() || host.name;
}

/** UniFi fingerprint artwork for common consumer devices we can identify from network evidence. */
export function deviceFingerprintId(host: Host): string | null {
  const evidence = host.evidence;
  const description = [
    host.name,
    host.vendor,
    host.device?.label,
    evidence.mdns_name,
    ...evidence.mdns_instances,
    evidence.mdns_model,
    evidence.upnp_manufacturer,
    evidence.upnp_model,
    evidence.upnp_friendly_name,
  ]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();

  if (/\bkindle fire\b/.test(description)) return "67";
  if (/\bkindle\b/.test(description)) return "41";
  if (/\biphone\b/.test(description)) return "91";
  if (/\bipad\b/.test(description)) return "92";
  if (/\bapple watch\b/.test(description)) return "54";
  if (/\bfire ?(?:tv|stick)\b/.test(description)) return "31";
  if (/\bchromecast\b/.test(description)) return "39";
  if (/\broku\b/.test(description)) return "27";
  if (/\bpixel\b/.test(description)) return "20";
  if (/\bgalaxy\b/.test(description)) return "2";
  if (/\bsonos\b/.test(description)) return "293";
  return findUniFiIcon([
    host.name,
    host.vendor ?? "",
    host.device?.label ?? "",
    evidence.mdns_name ?? "",
    ...evidence.mdns_instances,
    evidence.mdns_model ?? "",
    evidence.upnp_manufacturer ?? "",
    evidence.upnp_model ?? "",
    evidence.upnp_friendly_name ?? "",
  ]);
}

/** selfh.st artwork for hosts that are clearly named after a self-hosted app. */
export function deviceAppIcon(host: Host): string | null {
  const evidence = host.evidence;
  const description = [
    host.name,
    host.device?.label,
    evidence.mdns_name,
    ...evidence.mdns_instances,
    evidence.mdns_model,
    evidence.upnp_friendly_name,
    ...(host.metrics?.containers ?? []).flatMap((container) => [
      container.name,
      container.image,
    ]),
  ]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();

  const apps: [RegExp, string][] = [
    [/\bplex\b/, "plex"],
    [/\bjellyfin\b/, "jellyfin"],
    [/\bemby\b/, "emby"],
    [/\bhome[ -]?assistant\b/, "home-assistant"],
    [/\bimmich\b/, "immich"],
    [/\bnextcloud\b/, "nextcloud"],
    [/\bjellyseerr\b/, "jellyseerr"],
    [/\bsonarr\b/, "sonarr"],
    [/\bradarr\b/, "radarr"],
    [/\bprowlarr\b/, "prowlarr"],
    [/\bqbittorrent\b/, "qbittorrent"],
  ];
  return (
    apps.find(([pattern]) => pattern.test(description))?.[1] ??
    findSelfHostedIcon([
      host.name,
      host.device?.label ?? "",
      evidence.mdns_name ?? "",
      ...evidence.mdns_instances,
      evidence.mdns_model ?? "",
      evidence.upnp_friendly_name ?? "",
      ...(host.metrics?.containers ?? []).flatMap((container) => [
        container.name,
        container.image,
      ]),
    ])
  );
}

/** What the device is, as specifically as known ("Proxmox VE", "Dishwasher"), or null. */
export function deviceLabel(host: Host): string | null {
  if (host.device && host.device.kind !== "unknown") return host.device.label;
  const kind = deviceKind(host);
  return kind === "unknown" ? null : (BY_KIND.get(kind)?.label ?? null);
}

export function kindLabel(kind: string): string {
  return BY_KIND.get(kind)?.label ?? "Unknown device";
}

/** Confidence in words, for a guess the server made (not one the administrator set). */
export function confidenceLabel(confidence: number): string {
  if (confidence >= 85) return "High confidence";
  if (confidence >= 60) return "Likely";
  if (confidence >= 35) return "Possible";
  return "Low confidence";
}
