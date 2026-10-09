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

pub async fn availability(
    Extract(state): Extract<State>,
) -> ApiResult<axum::Json<hope_protocol::ReleaseAvailability>> {
    let directory = std::path::Path::new(&state.0.release_dir);
    let key_available = tokio::fs::metadata(directory.join("release-public.pem"))
        .await
        .is_ok_and(|meta| meta.is_file() && meta.len() > 0 && meta.len() <= 16384);
    let mut packaged_platforms = vec![];
    if key_available {
        for platform in [
            "linux-x86_64",
            "linux-aarch64",
            "macos-x86_64",
            "macos-aarch64",
        ] {
            let name = format!("hope-agent-{platform}");
            let binary = tokio::fs::metadata(directory.join(&name)).await;
            let signature = tokio::fs::metadata(directory.join(format!("{name}.sig"))).await;
            if binary.is_ok_and(|meta| meta.is_file() && meta.len() > 0)
                && signature
                    .is_ok_and(|meta| meta.is_file() && meta.len() > 0 && meta.len() <= 16384)
            {
                packaged_platforms.push(platform.into());
            }
        }
    }
    Ok(axum::Json(hope_protocol::ReleaseAvailability {
        packaged_platforms,
    }))
}
