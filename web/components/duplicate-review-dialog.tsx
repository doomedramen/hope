"use client";
import { useState } from "react";
import { GitMerge, RefreshCw } from "lucide-react";
import { api, post } from "@/lib/api";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";

type Candidate = {
  id: string;
  name: string;
  primary_ip: string | null;
  device_type: string | null;
  platform: string | null;
  source_count: number;
  last_seen: number | null;
  sources: string[];
};
type Group = {
  suggested_survivor_id: string;
  review_only?: boolean;
  candidates: Candidate[];
  evidence: { label: string; value: string }[];
};

export function DuplicateReviewDialog({ onMerged }: { onMerged: () => void }) {
  const [open, setOpen] = useState(false);
  const [groups, setGroups] = useState<Group[]>([]);
  const [survivors, setSurvivors] = useState<Record<number, string>>({});
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  async function load() {
    setLoading(true);
    setError("");
    try {
      const result = await api<{ groups: Group[] }>(
        "/devices/duplicate-groups",
      );
      setGroups(result.groups);
      setSurvivors(
        Object.fromEntries(
          result.groups.map((group, index) => [
            index,
            group.suggested_survivor_id,
          ]),
        ),
      );
      setSelected(
        new Set(
          result.groups
            .map((group, index) => (group.review_only ? -1 : index))
            .filter((index) => index >= 0),
        ),
      );
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }
  function changeOpen(next: boolean) {
    setOpen(next);
    if (next) void load();
  }
  async function reconcile() {
    const merges = [...selected]
      .map((index) => {
        const group = groups[index];
        const survivor = survivors[index];
        return {
          survivor_id: survivor,
          duplicate_ids: group.candidates
            .map((candidate) => candidate.id)
            .filter((id) => id !== survivor),
        };
      })
      .filter((merge) => merge.duplicate_ids.length > 0);
    if (!merges.length) return;
    setLoading(true);
    setError("");
    try {
      await post("/devices/duplicate-groups/reconcile", { merges });
      await load();
      onMerged();
    } catch (e) {
      setError((e as Error).message);
      setLoading(false);
    }
  }
  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogTrigger
        render={
          <Button variant="outline" size="sm">
            <GitMerge size={14} /> Review duplicates
          </Button>
        }
      />
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>Review duplicate devices</DialogTitle>
          <DialogDescription>
            Choose the canonical device to keep in each group. Selected groups
            are reconciled together; old device links resolve to the survivor.
          </DialogDescription>
        </DialogHeader>
        {error && (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        )}
        {loading && !groups.length ? (
          <p className="text-sm text-muted-foreground">
            Loading duplicate groups…
          </p>
        ) : groups.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No duplicate groups need review.
          </p>
        ) : (
          <div className="flex flex-col gap-3">
            {groups.map((group, index) => (
              <section
                key={group.candidates
                  .map((candidate) => candidate.id)
                  .join(":")}
                className="rounded-lg border p-3"
              >
                <label className="mb-2 flex items-center gap-2 text-sm font-medium">
                  <input
                    type="checkbox"
                    checked={selected.has(index)}
                    onChange={(event) =>
                      setSelected((old) => {
                        const next = new Set(old);
                        if (event.target.checked) next.add(index);
                        else next.delete(index);
                        return next;
                      })
                    }
                  />{" "}
                  Include this group
                </label>
                {group.review_only && (
                  <p className="mb-2 text-xs text-amber-700 dark:text-amber-300">
                    Review required: evidence is contextual and does not prove
                    device identity. Select this group only after checking the
                    candidates.
                  </p>
                )}
                {group.evidence.length > 0 && (
                  <p className="mb-3 text-xs text-muted-foreground">
                    Evidence:{" "}
                    {group.evidence
                      .map((item) => `${item.label}: ${item.value}`)
                      .join(" · ")}
                  </p>
                )}
                <div className="flex flex-col gap-2">
                  {group.candidates.map((candidate) => (
                    <label
                      key={candidate.id}
                      className="flex min-w-0 items-start gap-2 rounded-md bg-muted/40 p-2 text-sm"
                    >
                      <input
                        type="radio"
                        name={`survivor-${index}`}
                        checked={survivors[index] === candidate.id}
                        onChange={() =>
                          setSurvivors((old) => ({
                            ...old,
                            [index]: candidate.id,
                          }))
                        }
                      />
                      <span className="min-w-0 flex-1">
                        <strong className="break-words">
                          {candidate.name}
                        </strong>
                        <span className="block break-all text-xs text-muted-foreground">
                          {[
                            candidate.primary_ip,
                            candidate.platform,
                            candidate.device_type,
                            candidate.sources?.join(", "),
                          ]
                            .filter(Boolean)
                            .join(" · ")}
                        </span>
                      </span>
                      <span className="shrink-0 text-xs text-muted-foreground">
                        {candidate.source_count} sources
                      </span>
                    </label>
                  ))}
                </div>
              </section>
            ))}
          </div>
        )}
        <div className="flex flex-wrap justify-between gap-2">
          <Button
            variant="outline"
            disabled={loading}
            onClick={() => void load()}
          >
            <RefreshCw size={14} /> Refresh
          </Button>
          <Button
            disabled={loading || selected.size === 0}
            onClick={() => void reconcile()}
          >
            Reconcile {selected.size} groups
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
