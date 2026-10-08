use crate::{
    error::{ApiError, ApiResult},
    state::{State, hash, now, secret},
};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::{
    Json,
    extract::{Request, State as Extract},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use subtle::ConstantTimeEq;

pub async fn status(
    Extract(state): Extract<State>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(
        json!({"setup_required": state.meta("password_hash").await?.is_none(), "authenticated": valid_session(&state, &headers).await?}),
    ))
}

#[derive(Deserialize)]
pub struct Login {
    pub password: String,
    pub setup_token: Option<String>,
}

pub async fn setup(Extract(state): Extract<State>, Json(body): Json<Login>) -> ApiResult<Response> {
    let _lock = state.0.writes.lock().await;
    if state.meta("password_hash").await?.is_some() {
        return Err(ApiError::bad("Setup already completed"));
    }
    let expected = state
        .meta("setup_token_hash")
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    if !bool::from(
        expected
            .as_bytes()
            .ct_eq(hash(body.setup_token.as_deref().unwrap_or("")).as_bytes()),
    ) {
        return Err(ApiError::unauthorized());
    }
    if body.password.len() < 12 || body.password.len() > 256 {
        return Err(ApiError::bad(
            "Use a password between 12 and 256 characters",
        ));
    }
    let password = body.password;
    let encoded = tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(
                password.as_bytes(),
                &SaltString::generate(&mut rand::rngs::OsRng),
            )
            .map(|h| h.to_string())
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    })
    .await??;
    state.set_meta("password_hash", &encoded).await?;
    sqlx::query("DELETE FROM meta WHERE key='setup_token_hash'")
        .execute(&state.0.db)
        .await?;
    new_session(&state).await
}

pub async fn login(Extract(state): Extract<State>, Json(body): Json<Login>) -> ApiResult<Response> {
    if body.password.len() > 256 {
        return Err(ApiError::unauthorized());
    }
    // One administrator: a global bounded rate limit avoids trusting proxy-supplied IP headers.
    {
        let mut attempts = state.0.login_attempts.lock().await;
        let entry = attempts.entry("admin".into()).or_insert((0, now()));
        if now() - entry.1 > 60.0 {
            *entry = (0, now());
        }
        if entry.0 >= 10 {
            return Err(ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many attempts; wait one minute".into(),
            ));
        }
        entry.0 += 1;
    }
    let encoded = state
        .meta("password_hash")
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let valid = tokio::task::spawn_blocking(move || {
        PasswordHash::new(&encoded).is_ok_and(|parsed| {
            Argon2::default()
                .verify_password(body.password.as_bytes(), &parsed)
                .is_ok()
        })
    })
    .await?;
    if !valid {
        return Err(ApiError::unauthorized());
    }
    new_session(&state).await
}

async fn new_session(state: &State) -> ApiResult<Response> {
    let token = secret();
    sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
        .bind(now())
        .execute(&state.0.db)
        .await?;
    sqlx::query("INSERT INTO sessions(token_hash,expires_at) VALUES(?,?)")
        .bind(hash(&token))
        .bind(now() + 86400.0)
        .execute(&state.0.db)
        .await?;
    let cookie = format!(
        "hope_session={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=86400{}",
        if state.0.secure_cookie {
            "; Secure"
        } else {
            ""
        }
    );
    Ok((
        [(axum::http::header::SET_COOKIE, cookie)],
        Json(json!({"ok":true})),
    )
        .into_response())
}

pub fn session_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("cookie")?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|c| c.trim().strip_prefix("hope_session="))
}
async fn valid_session(state: &State, headers: &HeaderMap) -> ApiResult<bool> {
    let Some(token) = session_token(headers) else {
        return Ok(false);
    };
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sessions WHERE token_hash=? AND expires_at>?",
    )
    .bind(hash(token))
    .bind(now())
    .fetch_one(&state.0.db)
    .await?
        > 0)
}
pub async fn require_session(
    Extract(state): Extract<State>,
    request: Request,
    next: Next,
) -> ApiResult<Response> {
    if !valid_session(&state, request.headers()).await? {
        return Err(ApiError::unauthorized());
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    Ok(response)
}
pub async fn check_origin(
    Extract(state): Extract<State>,
    request: Request,
    next: Next,
) -> ApiResult<Response> {
    if !matches!(request.method().as_str(), "GET" | "HEAD" | "OPTIONS") {
        if let Some(origin) = request.headers().get("origin")
            && origin.to_str().ok() != Some(state.0.server_url.as_str()) {
                return Err(ApiError(
                    StatusCode::FORBIDDEN,
                    "Untrusted request origin".into(),
                ));
            }
        if request
            .headers()
            .get("sec-fetch-site")
            .is_some_and(|h| h == "cross-site")
        {
            return Err(ApiError(
                StatusCode::FORBIDDEN,
                "Cross-site request denied".into(),
            ));
        }
    }
    Ok(next.run(request).await)
}
pub async fn logout(Extract(state): Extract<State>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(token) = session_token(&headers) {
        sqlx::query("DELETE FROM sessions WHERE token_hash=?")
            .bind(hash(token))
            .execute(&state.0.db)
            .await?;
    }
    Ok((
        [(
            "set-cookie",
            "hope_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
        )],
        Json(json!({"ok":true})),
    )
        .into_response())
}
