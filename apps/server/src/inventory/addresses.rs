//! Address (IP) assignment. IP history is append-only: moving an IP never
//! mutates the old row, it closes it (`is_current = false`) and inserts a
//! new one (design §1) -- this is the mechanism behind the "device can
//! change IP without a duplicate device" acceptance gate.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use std::net::IpAddr;
use uuid::Uuid;

use crate::inventory::pagination::{ListParams, decode_cursor, effective_limit, encode_cursor};
use crate::state::AppState;

fn err(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg.into() })))
}

#[derive(Debug, Deserialize)]
pub struct AssignAddress {
    pub interface_id: Uuid,
    pub ip: String,
    pub address_type: Option<String>,
}

/// Assign `ip` as the current address for `interface_id`. If a different
/// IP is currently live on that interface, its row is closed
/// (`is_current = false`, `last_seen = now()`) rather than mutated in
/// place, and a new row is inserted -- history is never lost.
pub async fn assign_address_tx(
    pool: &PgPool,
    interface_id: Uuid,
    ip: &str,
    address_type: Option<&str>,
) -> sqlx::Result<Value> {
    let mut tx = pool.begin().await?;

    let row = assign_address_in_tx(&mut tx, interface_id, ip, address_type).await?;

    tx.commit().await?;
    Ok(row)
}

/// Assign `ip` inside an existing transaction. Refreshing an already-current
/// address updates its observation timestamp; moving an address closes its
/// old row and appends a new current row.
async fn assign_address_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    interface_id: Uuid,
    ip: &str,
    address_type: Option<&str>,
) -> sqlx::Result<Value> {
    let already_current: Option<(String,)> =
        sqlx::query_as("select host(ip) from addresses where interface_id = $1 and is_current")
            .bind(interface_id)
            .fetch_optional(&mut **tx)
            .await?;

    if let Some((current_ip,)) = &already_current {
        if current_ip == ip {
            let row: (Value,) = sqlx::query_as(
                "update addresses set last_seen = now(), version = version + 1, updated_at = now() \
                 where interface_id = $1 and is_current \
                 returning row_to_json(addresses.*)",
            )
            .bind(interface_id)
            .fetch_one(&mut **tx)
            .await?;
            return Ok(row.0);
        }

        sqlx::query(
            "update addresses set is_current = false, last_seen = now(), version = version + 1, updated_at = now() \
             where interface_id = $1 and is_current",
        )
        .bind(interface_id)
        .execute(&mut **tx)
        .await?;
    }

    let row: (Value,) = sqlx::query_as(
        "insert into addresses (interface_id, ip, address_type, is_current) \
         values ($1, $2::inet, coalesce($3, 'unknown'), true) \
         returning row_to_json(addresses.*)",
    )
    .bind(interface_id)
    .bind(ip)
    .bind(address_type)
    .fetch_one(&mut **tx)
    .await?;

    Ok(row.0)
}

/// Resolve the device currently owning `ip`, or create an unconfirmed
/// unknown device with one interface and current address for a newly scanned
/// address. The address lock prevents concurrent scans from creating two
/// owners for one current IP.
pub async fn ensure_scanned_address(pool: &PgPool, ip: IpAddr) -> sqlx::Result<Uuid> {
    let ip = ip.to_string();
    let mut tx = pool.begin().await?;

    sqlx::query("select pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(&ip)
        .execute(&mut *tx)
        .await?;

    let existing: Option<(Uuid, Uuid)> = sqlx::query_as(
        "select a.interface_id, d.id \
         from addresses a \
         join interfaces i on i.id = a.interface_id \
         join devices d on d.id = i.device_id \
         where a.ip = $1::inet and a.is_current \
         for update of a",
    )
    .bind(&ip)
    .fetch_optional(&mut *tx)
    .await?;

    let device_id = if let Some((interface_id, device_id)) = existing {
        assign_address_in_tx(&mut tx, interface_id, &ip, Some("unknown")).await?;
        resolve_device_in_tx(&mut tx, device_id).await?
    } else {
        let (device_id,): (Uuid,) = sqlx::query_as(
            "insert into devices (device_type, status) values ('unknown', 'active') returning id",
        )
        .fetch_one(&mut *tx)
        .await?;
        let (interface_id,): (Uuid,) =
            sqlx::query_as("insert into interfaces (device_id) values ($1) returning id")
                .bind(device_id)
                .fetch_one(&mut *tx)
                .await?;
        assign_address_in_tx(&mut tx, interface_id, &ip, Some("unknown")).await?;
        device_id
    };

    tx.commit().await?;
    Ok(device_id)
}

async fn resolve_device_in_tx(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> sqlx::Result<Uuid> {
    let mut current = id;
    loop {
        let next: Option<(Option<Uuid>,)> =
            sqlx::query_as("select canonical_of from devices where id = $1")
                .bind(current)
                .fetch_optional(&mut **tx)
                .await?;
        match next {
            Some((Some(canonical_of),)) if canonical_of != current => current = canonical_of,
            _ => return Ok(current),
        }
    }
}

pub async fn assign(
    State(state): State<AppState>,
    Json(req): Json<AssignAddress>,
) -> (StatusCode, Json<Value>) {
    match assign_address_tx(
        &state.pool,
        req.interface_id,
        &req.ip,
        req.address_type.as_deref(),
    )
    .await
    {
        Ok(v) => (StatusCode::CREATED, Json(v)),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct AddressListParams {
    pub interface_id: Option<Uuid>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<AddressListParams>,
) -> (StatusCode, Json<Value>) {
    let limit = effective_limit(params.limit);
    let cursor = decode_cursor(
        &ListParams {
            cursor: params.cursor,
            limit: params.limit,
        }
        .cursor,
    );

    let result = sqlx::query_as::<_, (Value,)>(
        "select row_to_json(t) from (select * from addresses \
            where ($1::uuid is null or interface_id = $1) \
              and ($2::timestamptz is null or (created_at, id) > ($2::timestamptz, $3)) \
            order by created_at, id limit $4) t",
    )
    .bind(params.interface_id)
    .bind(cursor.as_ref().map(|(c, _)| c.clone()))
    .bind(cursor.as_ref().map(|(_, id)| *id))
    .bind(limit)
    .fetch_all(&state.pool)
    .await;

    let rows = match result {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let items: Vec<Value> = rows.into_iter().map(|(v,)| v).collect();
    let next_cursor = items.last().and_then(|last| {
        let created_at = last.get("created_at")?.as_str()?;
        let id = last.get("id")?.as_str()?;
        Some(encode_cursor(created_at, Uuid::parse_str(id).ok()?))
    });

    (
        StatusCode::OK,
        Json(json!({ "items": items, "next_cursor": next_cursor })),
    )
}

#[cfg(test)]
mod gate_tests {
    //! Acceptance gate (spec §17 M1, design §7): "device can change IP
    //! without a duplicate device."

    use super::*;

    async fn pool_or_skip() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        Some(pool)
    }

    #[tokio::test]
    async fn ip_change_closes_old_address_and_keeps_one_device() {
        let Some(pool) = pool_or_skip().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        let (device_id,): (Uuid,) = sqlx::query_as(
            "insert into devices (device_type) values ('physical_host') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let (interface_id,): (Uuid,) = sqlx::query_as(
            "insert into interfaces (device_id, mac) values ($1, '00:11:22:33:44:55') returning id",
        )
        .bind(device_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assign_address_tx(&pool, interface_id, "10.0.0.5", Some("dhcp"))
            .await
            .unwrap();
        assign_address_tx(&pool, interface_id, "10.0.0.9", Some("dhcp"))
            .await
            .unwrap();

        let device_count: (i64,) = sqlx::query_as("select count(*) from devices where id = $1")
            .bind(device_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            device_count.0, 1,
            "no duplicate device created on IP change"
        );

        let current: (i64,) =
            sqlx::query_as("select count(*) from addresses where interface_id = $1 and is_current")
                .bind(interface_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(current.0, 1, "exactly one current address");

        let closed_old: (String, bool) = sqlx::query_as(
            "select host(ip), is_current from addresses where interface_id = $1 and ip = '10.0.0.5'::inet",
        )
        .bind(interface_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            !closed_old.1,
            "old address row closed, not deleted or mutated to the new IP"
        );

        let total: (i64,) =
            sqlx::query_as("select count(*) from addresses where interface_id = $1")
                .bind(interface_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            total.0, 2,
            "history preserved: old + new row, not one row overwritten"
        );
    }
}
