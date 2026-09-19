//! Change-event / audit-log recorder (design §5). Both tables are written
//! in the same transaction as the mutation that caused them —
//! application-level, not DB triggers, so it's plain testable Rust.

use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub struct Recorder;

impl Recorder {
    #[allow(clippy::too_many_arguments)]
    pub async fn record_change(
        tx: &mut Transaction<'_, Postgres>,
        entity_kind: &str,
        entity_id: Uuid,
        category: &str,
        severity: &str,
        before: Option<Value>,
        after: Option<Value>,
        evidence_source: Option<&str>,
    ) -> sqlx::Result<()> {
        sqlx::query(
            "insert into change_events \
                (entity_kind, entity_id, category, severity, before, after, evidence_source) \
             values ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(entity_kind)
        .bind(entity_id)
        .bind(category)
        .bind(severity)
        .bind(before)
        .bind(after)
        .bind(evidence_source)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_audit(
        tx: &mut Transaction<'_, Postgres>,
        actor_user_id: Option<Uuid>,
        actor_kind: &str,
        action: &str,
        target_kind: Option<&str>,
        target_id: Option<Uuid>,
        result: &str,
        detail: Option<Value>,
    ) -> sqlx::Result<()> {
        sqlx::query(
            "insert into audit_events \
                (actor_user_id, actor_kind, action, target_kind, target_id, result, detail) \
             values ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(actor_user_id)
        .bind(actor_kind)
        .bind(action)
        .bind(target_kind)
        .bind(target_id)
        .bind(result)
        .bind(detail)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }
}
