"use client";

import Image from "next/image";
import { useState } from "react";
import { Boxes } from "lucide-react";

const ICON_ALIASES: Record<string, string> = {
  homeassistant: "home-assistant",
  "immich-machine-learning": "immich",
  "immich-server": "immich",
  postgres: "postgresql",
};

function iconSlug(image: string): string | null {
  const withoutDigest = image.split("@", 1)[0];
  const withoutTag = withoutDigest.replace(/:[^/]+$/, "");
  const name = withoutTag.split("/").at(-1)?.toLowerCase();
  if (!name) return null;
  const slug = name.replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
  return ICON_ALIASES[slug] ?? slug;
}

export function ContainerAppIcon({
  image,
  name,
}: {
  image: string;
  name: string;
}) {
  const [failed, setFailed] = useState(false);
  const slug = iconSlug(image);
  if (!slug || failed) {
    return (
      <span className="container-app-icon" aria-hidden="true">
        <Boxes size={17} />
      </span>
    );
  }
  return (
    <span className="container-app-icon">
      <Image
        src={`https://cdn.jsdelivr.net/gh/selfhst/icons@main/png/${slug}.png`}
        width={34}
        height={34}
        alt={`${name} application icon`}
        unoptimized
        onError={() => setFailed(true)}
      />
    </span>
  );
}
