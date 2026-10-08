use crate::{
    error::{ApiError, ApiResult},
    state::State,
};
use axum::{extract::State as Extract, http::StatusCode, response::IntoResponse};

pub async fn installer(Extract(state): Extract<State>) -> ApiResult<impl IntoResponse> {
    let key = tokio::fs::read_to_string(
        std::path::Path::new(&state.0.release_dir).join("release-public.pem"),
    )
    .await
    .map_err(|_| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Signed agent releases are not packaged. Run scripts/package-agent.sh first.".into(),
        )
    })?;
    if !key.starts_with("-----BEGIN PUBLIC KEY-----") || key.len() > 16384 {
        return Err(ApiError::bad("Invalid release verification key"));
    }
    let script = include_str!("../../../scripts/install-agent.sh")
        .replace("@@HOPE_PUBLIC_KEY@@", key.trim());
    Ok((
        [
            ("content-type", "text/x-shellscript"),
            ("cache-control", "no-store"),
        ],
        script,
    ))
}
