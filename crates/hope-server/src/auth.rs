use crate::{
    error::{ApiError, ApiResult},
    limits::ClientKey,
    state::{State, hash, now, secret},
};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::{
    Json,
    extract::{Extension, Request, State as Extract},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use hope_protocol::{AuthStatus, OkResponse, PasswordChange};
use subtle::ConstantTimeEq;

pub async fn status(
    Extract(state): Extract<State>,
    headers: HeaderMap,
) -> ApiResult<Json<AuthStatus>> {
    Ok(Json(AuthStatus {
        setup_required: state.meta("password_hash").await?.is_none(),
        authenticated: valid_session(&state, &headers).await?,
    }))
}

pub use hope_protocol::LoginRequest as Login;
pub async fn setup(
    Extract(state): Extract<State>,
    Extension(client): Extension<ClientKey>,
    Json(body): Json<Login>,
) -> ApiResult<Response> {
    let setup_key = format!("setup:{}", client.0);
    limited(state.0.login_limit.check(&setup_key, now()))?;
    let _lock = state.0.writes.lock().await;
    if state.meta("password_hash").await?.is_some() {
        return Err(ApiError::bad("Setup already completed"));
    }
    let Some(expected) = state.meta("setup_token_hash").await? else {
        state.0.login_limit.hit(&setup_key, now());
        return Err(ApiError::unauthorized());
    };
    if !bool::from(
        expected
            .as_bytes()
            .ct_eq(hash(body.setup_token.as_deref().unwrap_or("")).as_bytes()),
    ) {
        state.0.login_limit.hit(&setup_key, now());
        return Err(ApiError::unauthorized());
    }
    if body.password.len() < 8 || body.password.len() > 256 {
        return Err(ApiError::bad("Use a password between 8 and 256 characters"));
    }
    let encoded = hash_password(&state, body.password).await?;
    state.set_meta("password_hash", &encoded).await?;
    sqlx::query("DELETE FROM meta WHERE key='setup_token_hash'")
        .execute(&state.0.db)
        .await?;
    new_session(&state).await
}

/// Map a limiter refusal to a bounded 429 that reveals nothing about secrets.
pub fn limited(result: Result<(), u64>) -> ApiResult<()> {
    result.map_err(|wait| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            format!("Too many attempts; retry in {wait} seconds"),
        )
    })
}

/// Bound concurrent password hashing. Exhaustion is transient (503 + Retry-After): it can delay a
/// login during a flood but never locks anyone out.
async fn hash_permit(state: &State) -> ApiResult<tokio::sync::SemaphorePermit<'_>> {
    match tokio::time::timeout(std::time::Duration::from_secs(2), state.0.hashing.acquire()).await {
        Ok(Ok(permit)) => Ok(permit),
        _ => Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Server busy; retry shortly".into(),
        )),
    }
}

pub async fn login(
    Extract(state): Extract<State>,
    Extension(client): Extension<ClientKey>,
    Json(body): Json<Login>,
) -> ApiResult<Response> {
    // Only failures count, per client address (see `HOPE_TRUSTED_PROXIES`); there is no global
    // login ceiling an attacker could fill to lock the administrator out.
    limited(state.0.login_limit.check(&client.0, now()))?;
    let fail = || {
        state.0.login_limit.hit(&client.0, now());
        ApiError::unauthorized()
    };
    if body.password.len() > 256 {
        return Err(fail());
    }
    if !verify_password(&state, body.password).await? {
        return Err(fail());
    }
    state.0.login_limit.clear(&client.0);
    new_session(&state).await
}

/// Whether `password` matches the stored administrator password (false when none is set).
async fn verify_password(state: &State, password: String) -> ApiResult<bool> {
    let Some(encoded) = state.meta("password_hash").await? else {
        return Ok(false);
    };
    let _permit = hash_permit(state).await?;
    Ok(tokio::task::spawn_blocking(move || {
        PasswordHash::new(&encoded).is_ok_and(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
    })
    .await?)
}

/// Argon2 PHC string for a new administrator password.
async fn hash_password(state: &State, password: String) -> ApiResult<String> {
    let _permit = hash_permit(state).await?;
    Ok(tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(
                password.as_bytes(),
                &SaltString::generate(&mut rand::rngs::OsRng),
            )
            .map(|h| h.to_string())
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    })
    .await??)
}

/// Change the administrator password. Requires the current password (a borrowed session alone is
/// not enough), shares the login limiter so it cannot be used to guess the password, and ends every
/// session, issuing a fresh one to this browser so the person changing it stays signed in.
pub async fn change_password(
    Extract(state): Extract<State>,
    Extension(client): Extension<ClientKey>,
    Json(body): Json<PasswordChange>,
) -> ApiResult<Response> {
    let key = format!("password:{}", client.0);
    limited(state.0.login_limit.check(&key, now()))?;
    let PasswordChange {
        current_password,
        new_password,
    } = body;
    if current_password.len() > 256 || !verify_password(&state, current_password.clone()).await? {
        state.0.login_limit.hit(&key, now());
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "The current password is incorrect".into(),
        ));
    }
    state.0.login_limit.clear(&key);
    if new_password.len() < 8 || new_password.len() > 256 {
        return Err(ApiError::bad("Use a password between 8 and 256 characters"));
    }
    if new_password == current_password {
        return Err(ApiError::bad(
            "Choose a new password that differs from the current one",
        ));
    }
    let encoded = hash_password(&state, new_password).await?;
    let _lock = state.0.writes.lock().await;
    state.set_meta("password_hash", &encoded).await?;
    sqlx::query("DELETE FROM sessions")
        .execute(&state.0.db)
        .await?;
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
        Json(OkResponse { ok: true }),
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
            && origin
                .to_str()
                .ok()
                .and_then(crate::state::normalize_origin)
                .as_deref()
                != Some(state.0.canonical_origin.as_str())
        {
            return Err(ApiError(
                StatusCode::FORBIDDEN,
                "Untrusted request origin".into(),
            ));
        }
        // Browsers always send this header; agents and curl do not. Anything that is not
        // same-origin (or a direct navigation) is refused, including same-site siblings.
        if request
            .headers()
            .get("sec-fetch-site")
            .is_some_and(|h| h != "same-origin" && h != "none")
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
    let secure = if state.0.secure_cookie {
        "; Secure"
    } else {
        ""
    };
    if let Some(token) = session_token(&headers) {
        sqlx::query("DELETE FROM sessions WHERE token_hash=?")
            .bind(hash(token))
            .execute(&state.0.db)
            .await?;
    }
    Ok((
        [(
            "set-cookie",
            format!("hope_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0{secure}"),
        )],
        Json(OkResponse { ok: true }),
    )
        .into_response())
}
