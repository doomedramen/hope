import { Badge } from "@/components/ui/badge";
import { formatDate, labelize, shortId } from "@/lib/format";
import type { MaintenanceConflict } from "@/lib/api";

export function MaintenanceConflictList({
  conflicts,
  compact = false,
}: {
  conflicts: MaintenanceConflict[];
  compact?: boolean;
}) {
  if (conflicts.length === 0) return null;

  return (
    <div className="flex flex-col gap-3" role="list">
      {conflicts.map((conflict, index) => (
        <div
          className="rounded-lg border bg-background p-3 text-sm"
          key={`${conflict.occurrence_id}-${conflict.conflicting_occurrence_id}-${index}`}
          role="listitem"
        >
          <div className="flex flex-wrap items-center gap-2">
            <Badge variant="destructive">
              {labelize(conflict.relationship)}
            </Badge>
            <span className="text-xs text-muted-foreground">
              {formatDate(conflict.overlap_start)} –{" "}
              {formatDate(conflict.overlap_end)}
            </span>
          </div>
          <p className="mt-2 leading-relaxed">{conflict.reason}</p>
          {!compact ? (
            <p className="mt-1 text-muted-foreground">{conflict.suggestion}</p>
          ) : null}
          <p className="mt-2 text-xs text-muted-foreground">
            Conflicting event{" "}
            <span className="font-mono">
              {shortId(conflict.conflicting_event_id)}
            </span>
            {conflict.suggested_move?.start_after ? (
              <>
                {" "}
                · Suggested start after{" "}
                {formatDate(conflict.suggested_move.start_after)}
              </>
            ) : null}
          </p>
        </div>
      ))}
    </div>
  );
}
