import catalog from "@/data/icon-catalog.json";

type AppIcon = (typeof catalog.apps)[number];
type UniFiIcon = (typeof catalog.unifi)[number];

const APPS: Array<AppIcon & { tokens: string[]; normalized: string }> =
  catalog.apps.map((icon) => ({ ...icon, ...prepare(icon.name) }));
const UNIFI: Array<UniFiIcon & { tokens: string[]; normalized: string }> =
  catalog.unifi.map((icon) => ({ ...icon, ...prepare(icon.name) }));
const APP_TOKENS = tokenIndex(APPS);
const UNIFI_TOKENS = tokenIndex(UNIFI);
const ICON_OPTIONS: IconOption[] = [
  ...APPS.map((icon) => ({
    key: `selfhst:${icon.slug}`,
    name: icon.name,
    source: "selfhst" as const,
  })),
  ...UNIFI.map((icon) => ({
    key: `unifi:${icon.id}`,
    name: icon.name,
    source: "unifi" as const,
  })),
].sort((left, right) => left.name.localeCompare(right.name));

const GENERIC = new Set([
  "access",
  "amazon",
  "apple",
  "computer",
  "device",
  "google",
  "host",
  "machine",
  "microsoft",
  "network",
  "point",
  "samsung",
  "server",
  "smart",
  "unknown",
  "virtual",
]);

function prepare(value: string) {
  const tokens = value.toLowerCase().match(/[a-z0-9]+/g) ?? [];
  return { tokens, normalized: tokens.join("") };
}

function tokenIndex<T extends { tokens: string[] }>(entries: T[]) {
  const index = new Map<string, T[]>();
  for (const entry of entries) {
    for (const token of new Set(entry.tokens)) {
      for (const key of [token, ...deletionKeys(token)]) {
        const matches = index.get(key) ?? [];
        matches.push(entry);
        index.set(key, matches);
      }
    }
  }
  return index;
}

function deletionKeys(token: string): string[] {
  if (token.length < 5) return [];
  return Array.from(
    { length: token.length },
    (_, index) => token.slice(0, index) + token.slice(index + 1),
  );
}

function containsSequence(haystack: string[], needle: string[]): boolean {
  return (
    needle.length > 0 &&
    haystack.some((_, index) =>
      needle.every((token, offset) => haystack[index + offset] === token),
    )
  );
}

function editDistance(left: string, right: string): number {
  const row = Array.from({ length: right.length + 1 }, (_, index) => index);
  for (let i = 1; i <= left.length; i += 1) {
    let diagonal = row[0];
    row[0] = i;
    for (let j = 1; j <= right.length; j += 1) {
      const above = row[j];
      row[j] = Math.min(
        row[j] + 1,
        row[j - 1] + 1,
        diagonal + (left[i - 1] === right[j - 1] ? 0 : 1),
      );
      diagonal = above;
    }
  }
  return row[right.length];
}

function editDistanceWithSwap(left: string, right: string): number {
  const distance = editDistance(left, right);
  if (distance <= 1 || left.length !== right.length) return distance;
  const differences = Array.from(left, (_, index) => index).filter(
    (index) => left[index] !== right[index],
  );
  if (
    differences.length === 2 &&
    differences[1] === differences[0] + 1 &&
    left[differences[0]] === right[differences[1]] &&
    left[differences[1]] === right[differences[0]]
  ) {
    return 1;
  }
  return distance;
}

function score(
  source: string[],
  candidate: string[],
  normalized: string,
): number {
  const useful = source.filter((token) => !GENERIC.has(token));
  if (useful.length === 0) return 0;
  const sourceName = useful.join("");
  if (sourceName === normalized) return 1;
  if (containsSequence(useful, candidate))
    return 0.98 - (useful.length - candidate.length) * 0.002;
  if (containsSequence(candidate, useful))
    return 0.94 - (candidate.length - useful.length) * 0.015;

  const exact = useful.filter((token) => candidate.includes(token)).length;
  const fuzzy = useful.filter((token) =>
    candidate.some((word) => {
      if (token.length < 4 || word.length < 4) return false;
      const distance = editDistanceWithSwap(token, word);
      return (
        distance > 0 &&
        distance <= 1 &&
        distance / Math.max(token.length, word.length) <= 0.25
      );
    }),
  ).length;
  const matched = exact + fuzzy;
  if (matched === 0) return 0;
  return (matched / Math.max(useful.length, candidate.length)) * 0.88;
}

function bestMatch<T extends { tokens: string[]; normalized: string }>(
  inputs: string[],
  index: Map<string, T[]>,
): T | null {
  const sources = inputs.map(prepare).filter(({ tokens }) => tokens.length > 0);
  if (sources.length === 0) return null;
  const candidates = new Set<T>();
  for (const { tokens } of sources) {
    for (const token of tokens) {
      for (const candidate of index.get(token) ?? []) candidates.add(candidate);
      for (const key of deletionKeys(token)) {
        for (const candidate of index.get(key) ?? []) candidates.add(candidate);
      }
    }
  }
  const ranked = [...candidates]
    .map((entry) => ({
      entry,
      score: Math.max(
        ...sources.map(({ tokens }) =>
          score(tokens, entry.tokens, entry.normalized),
        ),
      ),
    }))
    .filter(({ score: value }) => value >= 0.86)
    .sort(
      (left, right) =>
        right.score - left.score ||
        left.entry.tokens.length - right.entry.tokens.length,
    );
  if (!ranked[0]) return null;
  if (ranked[1] && ranked[0].score - ranked[1].score < 0.015) return null;
  return ranked[0].entry;
}

export function findSelfHostedIcon(inputs: string[]): string | null {
  return bestMatch(inputs, APP_TOKENS)?.slug ?? null;
}

export function findUniFiIcon(inputs: string[]): string | null {
  return bestMatch(inputs, UNIFI_TOKENS)?.id ?? null;
}

export type IconOption = {
  key: string;
  name: string;
  source: "selfhst" | "unifi";
};

export function iconOption(key: string | null | undefined): IconOption | null {
  if (!key) return null;
  const [source, id] = key.split(":", 2);
  if (source === "selfhst") {
    const icon = APPS.find((entry) => entry.slug === id);
    return icon ? { key, name: icon.name, source } : null;
  }
  if (source === "unifi") {
    const icon = UNIFI.find((entry) => entry.id === id);
    return icon ? { key, name: icon.name, source } : null;
  }
  return null;
}

/** Search both checked-in icon catalogs for the host settings picker. */
export function searchIcons(query: string, limit = 20): IconOption[] {
  const { tokens } = prepare(query);
  if (tokens.length === 0) return ICON_OPTIONS.slice(0, limit);
  const candidates = new Set<(typeof APPS)[number] | (typeof UNIFI)[number]>();
  for (const token of tokens) {
    for (const candidate of APP_TOKENS.get(token) ?? [])
      candidates.add(candidate);
    for (const candidate of UNIFI_TOKENS.get(token) ?? [])
      candidates.add(candidate);
    for (const key of deletionKeys(token)) {
      for (const candidate of APP_TOKENS.get(key) ?? [])
        candidates.add(candidate);
      for (const candidate of UNIFI_TOKENS.get(key) ?? [])
        candidates.add(candidate);
    }
  }
  const normalizedQuery = tokens.join("");
  const ranked = [...candidates]
    .map((icon) => ({
      icon,
      score: score(tokens, icon.tokens, icon.normalized),
    }))
    .filter(
      ({ icon, score: value }) =>
        value >= 0.2 || icon.normalized.includes(normalizedQuery),
    )
    .sort(
      (left, right) =>
        right.score - left.score ||
        left.icon.name.localeCompare(right.icon.name),
    )
    .slice(0, limit);
  return ranked.map(({ icon }) => ({
    key: "slug" in icon ? `selfhst:${icon.slug}` : `unifi:${icon.id}`,
    name: icon.name,
    source: "slug" in icon ? "selfhst" : "unifi",
  }));
}
