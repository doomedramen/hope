//! Release serving: cache headers on the manifest and artifacts, and the installer's availability.
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use hope_protocol::Capabilities;
use hope_server::{router, state::State};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn app(release_dir: &std::path::Path) -> (tempfile::TempDir, axum::Router) {
    let data = tempfile::tempdir().expect("data");
    let state = State::open(
        data.path().join("test.db").to_str().expect("path"),
        "http://localhost:8080".into(),
        release_dir.to_str().expect("path").into(),
        Capabilities {
            platform: "test".into(),
            discovery_mode: "limited".into(),
            arp: false,
            icmp: false,
            limitations: vec![],
        },
    )
    .await
    .expect("state");
    (data, router(state, "web/out"))
}

async fn get(app: &axum::Router, path: &str) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let (status, headers) = (response.status(), response.headers().clone());
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, headers, body.to_vec())
}

#[tokio::test]
async fn manifest_and_signature_are_never_cached_and_artifacts_revalidate() {
    let dir = tempfile::tempdir().expect("dir");
    for (name, data) in [
        ("manifest.json", b"{}".as_slice()),
        ("manifest.json.sig", b"sig"),
        ("release-public.pem", b"pem"),
        ("hope-agent-linux-x86_64", b"binary"),
    ] {
        std::fs::write(dir.path().join(name), data).expect("write");
    }
    let (_data, app) = app(dir.path()).await;
    for name in ["manifest.json", "manifest.json.sig", "release-public.pem"] {
        let (status, headers, _) = get(&app, &format!("/agent/releases/{name}")).await;
        assert_eq!(status, StatusCode::OK, "{name}");
        assert_eq!(headers["cache-control"], "no-store", "{name}");
        assert_eq!(headers["x-content-type-options"], "nosniff");
    }
    let (status, headers, body) = get(&app, "/agent/releases/hope-agent-linux-x86_64").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"binary");
    assert_eq!(headers["cache-control"], "no-cache");
    let (status, headers, _) = get(&app, "/agent/releases/hope-agent-macos-aarch64").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(headers["cache-control"], "no-cache");
}

#[tokio::test]
async fn installer_requires_the_published_public_key() {
    let dir = tempfile::tempdir().expect("dir");
    let (_data, app) = app(dir.path()).await;
    let (status, _, body) = get(&app, "/agent/install").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(String::from_utf8_lossy(&body).contains("assemble-release.sh"));
    std::fs::write(
        dir.path().join("release-public.pem"),
        "-----BEGIN PUBLIC KEY-----\nabc\n-----END PUBLIC KEY-----\n",
    )
    .expect("key");
    let (status, _, body) = get(&app, "/agent/install").await;
    assert_eq!(status, StatusCode::OK);
    let script = String::from_utf8(body).expect("script");
    assert!(script.contains("abc") && !script.contains("@@HOPE_PUBLIC_KEY@@"));
}

#[tokio::test]
async fn artifacts_are_served_from_the_current_release_and_follow_swaps() {
    let root = tempfile::tempdir().expect("dir");
    for (name, body) in [("1.0.0-aaaa", "one"), ("1.1.0-bbbb", "two")] {
        std::fs::create_dir(root.path().join(name)).expect("mkdir");
        std::fs::write(root.path().join(name).join("manifest.json"), body).expect("write");
    }
    let current = root.path().join("current");
    std::os::unix::fs::symlink("1.0.0-aaaa", &current).expect("link");
    let (_data, app) = app(root.path()).await;
    assert_eq!(get(&app, "/agent/releases/manifest.json").await.2, b"one");
    // Atomic swap as assemble-release.sh does it: new symlink renamed over `current`.
    std::os::unix::fs::symlink("1.1.0-bbbb", root.path().join(".tmp")).expect("link");
    std::fs::rename(root.path().join(".tmp"), &current).expect("swap");
    let (status, headers, body) = get(&app, "/agent/releases/manifest.json").await;
    assert_eq!((status, body), (StatusCode::OK, b"two".to_vec()));
    assert_eq!(headers["cache-control"], "no-store");
    // Sibling release directories are not reachable by name.
    assert_eq!(
        get(&app, "/agent/releases/1.0.0-aaaa/manifest.json")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}
