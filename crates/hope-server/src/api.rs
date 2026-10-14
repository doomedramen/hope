use crate::{
    error::{ApiError, ApiResult},
    scanner,
    state::State,
};
use axum::{
    Json,
    extract::{Path, State as Extract},
    response::{
        Sse,
        sse::{Event, KeepAlive},
    },
};
use futures::StreamExt;
use hope_protocol::OkResponse;
use hope_protocol::{Bootstrap, Host, MapState, Network};
use sqlx::Row;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

pub async fn networks(state: &State) -> ApiResult<Vec<Network>> {
    Ok(sqlx::query("SELECT * FROM networks ORDER BY name")
        .fetch_all(&state.0.db)
        .await?
        .into_iter()
        .map(|r| Network {
            id: r.get("id"),
            name: r.get("name"),
            cidr: r.get("cidr"),
            enabled: r.get("enabled"),
        })
        .collect())
}
pub async fn matching_networks(state: &State, addresses: &[String]) -> ApiResult<Vec<String>> {
    Ok(networks(state)
        .await?
        .into_iter()
        .filter(|n| {
            n.cidr.parse::<ipnet::Ipv4Net>().is_ok_and(|net| {
                addresses.iter().any(|s| {
                    s.parse::<std::net::Ipv4Addr>()
                        .is_ok_and(|ip| net.contains(&ip))
                })
            })
        })
        .map(|n| n.id)
        .collect())
}
pub async fn bootstrap(Extract(state): Extract<State>) -> ApiResult<Json<Bootstrap>> {
    Ok(Json(Bootstrap {
        networks: networks(&state).await?,
        hosts: state.hosts().await?,
        scans: scanner::jobs(&state).await?,
        map: state
            .meta("map")
            .await?
            .map(|v| serde_json::from_str(&v))
            .transpose()?
            .unwrap_or_default(),
        capabilities: {
            let mut capabilities = state.0.capabilities.clone();
            capabilities.arp = capabilities.arp && state.0.scheduler.arp.available();
            capabilities
        },
        server_url: state.0.server_url.clone(),
    }))
}
pub use hope_protocol::NetworkInput;
pub async fn add_network(
    Extract(state): Extract<State>,
    Json(body): Json<NetworkInput>,
) -> ApiResult<Json<Network>> {
    let _guard = state.0.writes.lock().await;
    if body.name.trim().is_empty() || body.name.len() > 120 {
        return Err(ApiError::bad("Network name must contain 1–120 characters"));
    }
    if networks(&state).await?.len() >= 32 {
        return Err(ApiError::bad("At most 32 ranges are supported"));
    }
    let cidr = scanner::parse_range(&body.cidr)?.to_string();
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM networks WHERE cidr=?")
        .bind(&cidr)
        .fetch_one(&state.0.db)
        .await?;
    if existing > 0 {
        return Err(ApiError::bad("This range already exists"));
    }
    let network = Network {
        id: Uuid::new_v4().to_string(),
        name: body.name.trim().into(),
        cidr,
        enabled: body.enabled.unwrap_or(true),
    };
    sqlx::query("INSERT INTO networks(id,name,cidr,enabled) VALUES(?,?,?,?)")
        .bind(&network.id)
        .bind(&network.name)
        .bind(&network.cidr)
        .bind(network.enabled)
        .execute(&state.0.db)
        .await?;
    for mut host in state.hosts().await? {
        host.network_ids = matching_networks(&state, &host.addresses).await?;
        state.save_host(&host).await?;
    }
    state.changed();
    Ok(Json(network))
}
pub use hope_protocol::NetworkUpdate;
pub async fn update_network(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
    Json(body): Json<NetworkUpdate>,
) -> ApiResult<Json<OkResponse>> {
    sqlx::query("UPDATE networks SET enabled=? WHERE id=?")
        .bind(body.enabled)
        .bind(id)
        .execute(&state.0.db)
        .await?;
    state.changed();
    Ok(Json(OkResponse { ok: true }))
}
pub async fn start_scan(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
) -> ApiResult<Json<hope_protocol::ScanJob>> {
    let network = networks(&state)
        .await?
        .into_iter()
        .find(|n| n.id == id)
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(scanner::start(&state, &network).await?))
}
pub async fn cancel_scan(
    Extract(state): Extract<State>,
    Path(id): Path<String>,
) -> ApiResult<Json<OkResponse>> {
    scanner::cancel(&state, &id).await?;
    Ok(Json(OkResponse { ok: true }))
}
pub async fn map(
    Extract(state): Extract<State>,
    Json(mut map): Json<MapState>,
) -> ApiResult<Json<MapState>> {
    let _guard = state.0.writes.lock().await;
    let current: MapState = state
        .meta("map")
        .await?
        .map(|value| serde_json::from_str(&value))
        .transpose()?
        .unwrap_or_default();
    if map.revision != current.revision {
        return Err(ApiError(
            axum::http::StatusCode::CONFLICT,
            "Map changed; reload before replacing the layout".into(),
        ));
    }
    map.revision = next_revision(current.revision)?;
    validate_map(&state, &map).await?;
    state.set_meta("map", &serde_json::to_string(&map)?).await?;
    state.changed();
    Ok(Json(map))
}
fn next_revision(revision: u64) -> ApiResult<u64> {
    if revision >= 9_007_199_254_740_991 {
        return Err(ApiError::bad("Map revision limit reached"));
    }
    revision
        .checked_add(1)
        .ok_or_else(|| ApiError::bad("Map revision limit reached"))
}
async fn validate_map(state: &State, map: &MapState) -> ApiResult<()> {
    if map.positions.len() > 2000
        || map.links.len() > 2000
        || map
            .positions
            .iter()
            .any(|p| !p.x.is_finite() || !p.y.is_finite() || p.x.abs() > 1e6 || p.y.abs() > 1e6)
        || map
            .links
            .iter()
            .any(|l| l.label.len() > 120 || l.source == l.target)
    {
        return Err(ApiError::bad("Invalid map layout"));
    }
    let ids: Vec<_> = state.hosts().await?.into_iter().map(|h| h.id).collect();
    if map
        .links
        .iter()
        .any(|l| !ids.contains(&l.source) || !ids.contains(&l.target))
    {
        return Err(ApiError::bad("Manual links must connect existing hosts"));
    }
    if map
        .positions
        .iter()
        .any(|p| p.id.is_empty() || p.id.len() > 255)
        || map
            .links
            .iter()
            .any(|l| l.id.is_empty() || l.id.len() > 128)
        || map
            .positions
            .iter()
            .enumerate()
            .any(|(i, p)| map.positions[..i].iter().any(|other| other.id == p.id))
        || map
            .links
            .iter()
            .enumerate()
            .any(|(i, l)| map.links[..i].iter().any(|other| other.id == l.id))
    {
        return Err(ApiError::bad("Invalid or duplicate map identifiers"));
    }
    Ok(())
}
pub async fn edit_map(
    Extract(state): Extract<State>,
    Json(edit): Json<hope_protocol::MapEdit>,
) -> ApiResult<Json<MapState>> {
    let _guard = state.0.writes.lock().await;
    let mut map: MapState = state
        .meta("map")
        .await?
        .map(|value| serde_json::from_str(&value))
        .transpose()?
        .unwrap_or_default();
    match edit {
        hope_protocol::MapEdit::SetPosition { position } => {
            map.positions.retain(|p| p.id != position.id);
            map.positions.push(position);
        }
        hope_protocol::MapEdit::AddLink { link } => {
            if let Some(existing) = map.links.iter().find(|l| l.id == link.id) {
                if existing.source != link.source
                    || existing.target != link.target
                    || existing.label != link.label
                {
                    return Err(ApiError::bad("This map link identifier already exists"));
                }
            } else {
                map.links.push(link);
            }
        }
        hope_protocol::MapEdit::RemoveLink { id } => {
            map.links.retain(|l| l.id != id);
        }
    }
    map.revision = next_revision(map.revision)?;
    validate_map(&state, &map).await?;
    state.set_meta("map", &serde_json::to_string(&map)?).await?;
    state.changed();
    Ok(Json(map))
}
pub use hope_protocol::MergeRequest as Merge;
pub async fn merge(
    Extract(state): Extract<State>,
    Path(target): Path<String>,
    Json(body): Json<Merge>,
) -> ApiResult<Json<Host>> {
    let _guard = state.0.writes.lock().await;
    if target == body.source_id {
        return Err(ApiError::bad("Select two different hosts"));
    }
    let mut target = state.host(&target).await?.ok_or_else(ApiError::not_found)?;
    let source = state
        .host(&body.source_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if target.agent_id.is_some() && source.agent_id.is_some() {
        return Err(ApiError::bad("Cannot merge two enrolled agents"));
    }
    if target.agent_id.is_none() && source.agent_id.is_some() {
        return Err(ApiError::bad(
            "Select the enrolled agent as the destination",
        ));
    }
    for address in source.addresses {
        if !target.addresses.contains(&address) {
            target.addresses.push(address);
        }
    }
    for mac in source.macs {
        if !target.macs.contains(&mac) {
            target.macs.push(mac);
        }
    }
    for service in source.services {
        if !target.services.iter().any(|s| s.port == service.port) {
            target.services.push(service);
        }
    }
    target.network_ids = matching_networks(&state, &target.addresses).await?;
    let mut map: MapState = state
        .meta("map")
        .await?
        .map(|v| serde_json::from_str(&v))
        .transpose()?
        .unwrap_or_default();
    for link in &mut map.links {
        if link.source == source.id {
            link.source = target.id.clone();
        }
        if link.target == source.id {
            link.target = target.id.clone();
        }
    }
    map.links.retain(|l| l.source != l.target);
    map.positions.retain(|p| {
        p.id != source.id && p.id.rsplit_once(':').is_none_or(|(_, id)| id != source.id)
    });
    map.revision = next_revision(map.revision)?;
    let mut tx = state.0.db.begin().await?;
    sqlx::query("UPDATE hosts SET body=? WHERE id=?")
        .bind(serde_json::to_string(&target)?)
        .bind(&target.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM hosts WHERE id=?")
        .bind(source.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO meta(key,value) VALUES('map',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value").bind(serde_json::to_string(&map)?).execute(&mut *tx).await?;
    tx.commit().await?;
    state.changed();
    Ok(Json(target))
}
/// Server-sent change notifications. A receiver that falls behind the broadcast buffer gets a
/// `resync` event (the client reloads everything) instead of silently missing changes. The stream
/// ends when the server shuts down, after ten minutes, or when the client disconnects; dropping
/// the response drops the broadcast receiver.
pub async fn events(
    Extract(state): Extract<State>,
) -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let stream = BroadcastStream::new(state.0.events.subscribe())
        .map(|message| {
            Ok(match message {
                Ok(_) => Event::default().event("refresh").data("{}"),
                Err(_) => Event::default().event("resync").data("{}"),
            })
        })
        .take_until(tokio::time::sleep(std::time::Duration::from_secs(600)))
        .take_until(state.0.shutdown.clone().cancelled_owned());
    Sse::new(stream).keep_alive(KeepAlive::default())
}
