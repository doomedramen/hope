import catalog from "@/data/icon-catalog.json";

type AppIcon = (typeof catalog.apps)[number];
type UniFiIcon = (typeof catalog.unifi)[number];

const APPS: Array<AppIcon & { tokens: string[]; normalized: string }> =
  catalog.apps.map((icon) => ({ ...icon, ...prepare(icon.name) }));
const UNIFI: Array<UniFiIcon & { tokens: string[]; normalized: string }> =
  catalog.unifi.map((icon) => ({ ...icon, ...prepare(icon.name) }));
const APP_TOKENS = tokenIndex(APPS);
const UNIFI_TOKENS = tokenIndex(UNIFI);

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
      if (token.length < 5 || word.length < 5) return false;
      const distance = editDistance(token, word);
      return (
        distance > 0 &&
        distance <= 1 &&
        distance / Math.max(token.length, word.length) <= 0.2
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
