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
    let id = "1.0.0-0123456789ab";
    std::fs::create_dir(dir.path().join(id)).unwrap();
    std::os::unix::fs::symlink(id, dir.path().join("current")).unwrap();
    let (_data, app) = app(dir.path()).await;
    let (status, _, body) = get(&app, "/agent/install").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(String::from_utf8_lossy(&body).contains("assemble-release.sh"));
    std::fs::write(
        dir.path().join(id).join("release-public.pem"),
        "-----BEGIN PUBLIC KEY-----\nabc\n-----END PUBLIC KEY-----\n",
    )
    .expect("key");
    let (status, _, body) = get(&app, "/agent/install").await;
    assert_eq!(status, StatusCode::OK);
    let script = String::from_utf8(body).expect("script");
    assert!(script.contains("abc") && !script.contains("@@HOPE_PUBLIC_KEY@@"));
    assert!(
        script.contains(&format!("release_id='{id}'")) && !script.contains("@@HOPE_RELEASE_ID@@")
    );
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

fn swap(root: &std::path::Path, id: &str) {
    std::os::unix::fs::symlink(id, root.join(".next")).unwrap();
    std::fs::rename(root.join(".next"), root.join("current")).unwrap();
}

#[tokio::test]
async fn pinned_downloads_stay_on_one_release_through_publish_and_key_rotation() {
    let root = tempfile::tempdir().unwrap();
    let first = "1.0.0-aaaaaaaaaaaa";
    let second = "1.1.0-bbbbbbbbbbbb";
    let files = [
        "manifest.json",
        "manifest.json.sig",
        "hope-agent-linux-x86_64",
        "hope-agent-linux-x86_64.sig",
        "release-public.pem",
    ];
    for (id, label) in [(first, "first"), (second, "second")] {
        std::fs::create_dir(root.path().join(id)).unwrap();
        for file in files {
            let content = if file == "release-public.pem" {
                format!("-----BEGIN PUBLIC KEY-----\n{label}\n-----END PUBLIC KEY-----\n")
            } else {
                format!("{label}-{file}")
            };
            std::fs::write(root.path().join(id).join(file), content).unwrap();
        }
    }
    swap(root.path(), first);
    let (_data, app) = app(root.path()).await;
    let (status, _, installer) = get(&app, "/agent/install").await;
    assert_eq!(status, StatusCode::OK);
    let installer = String::from_utf8(installer).unwrap();
    assert!(installer.contains(&format!("release_id='{first}'")));
    assert!(installer.contains("\nfirst\n"));
    assert_eq!(
        get(&app, "/agent/releases/current-id").await.2,
        format!("{first}\n").as_bytes()
    );
    // Publish between every download step; the directory selected by the installer stays fixed.
    for file in files {
        swap(root.path(), second);
        let (status, headers, body) =
            get(&app, &format!("/agent/releases/by-id/{first}/{file}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            std::fs::read(root.path().join(first).join(file)).unwrap()
        );
        assert_eq!(headers["cache-control"], "public, max-age=86400, immutable");
        swap(root.path(), first);
    }
    swap(root.path(), second);
    let (status, headers, selected) = get(&app, "/agent/releases/current-id").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(selected, format!("{second}\n").as_bytes());
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(
        get(&app, "/agent/releases/manifest.json").await.2,
        b"second-manifest.json"
    );
}

#[tokio::test]
async fn pinned_namespace_refuses_aliases_symlinks_traversal_and_unpublished_files() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let id = "1.0.0-0123456789ab";
    let directory = root.path().join(id);
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("private.pem"), "secret").unwrap();
    std::fs::write(outside.path().join("manifest.json"), "outside").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("manifest.json"),
        directory.join("manifest.json"),
    )
    .unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("1.0.0-aaaaaaaaaaaa")).unwrap();
    swap(root.path(), id);
    let (_data, app) = app(root.path()).await;
    for path in [
        format!("/agent/releases/by-id/{id}/private.pem"),
        format!("/agent/releases/by-id/{id}/manifest.json"),
        "/agent/releases/by-id/1.0.0-aaaaaaaaaaaa/manifest.json".into(),
        "/agent/releases/by-id/current/manifest.json".into(),
        "/agent/releases/by-id/%2e%2e/manifest.json".into(),
        "/agent/releases/by-id/1.0.0-bad/manifest.json".into(),
        format!("/agent/releases/by-id/{id}/%2e%2e%2fprivate.pem"),
    ] {
        assert_eq!(get(&app, &path).await.0, StatusCode::NOT_FOUND, "{path}");
    }
    swap(root.path(), "1.0.0-aaaaaaaaaaaa");
    assert_eq!(
        get(&app, "/agent/releases/current-id").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn a_flat_directory_does_not_offer_an_installation_that_cannot_be_pinned() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("release-public.pem"),
        "-----BEGIN PUBLIC KEY-----\nx\n-----END PUBLIC KEY-----",
    )
    .unwrap();
    let (_data, app) = app(root.path()).await;
    assert_eq!(
        get(&app, "/agent/install").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        get(&app, "/agent/releases/current-id").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        get(&app, "/agent/releases/release-public.pem").await.0,
        StatusCode::OK
    );
}
