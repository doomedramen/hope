//! Migration upgrade-path test (migrations/README.md, M1/M4 gate follow-up):
//! apply the M0 schema (migrations 0001-0003) as if already deployed,
//! insert sample data, then apply all later migrations on top, and assert the
//! M0 data survived untouched.
//!
//! DB-gated: requires `DATABASE_URL` pointing at a throwaway Postgres 17,
//! e.g.
//! `docker run --rm -d -p 55432:5432 -e POSTGRES_PASSWORD=dev postgres:17`
//! then `DATABASE_URL=postgres://postgres:dev@localhost:55432/postgres`.
//! Skipped (not failed) when `DATABASE_URL` is unset, matching this
//! workspace's other DB-gated tests.

use std::path::PathBuf;

use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;

#[tokio::test]
async fn m0_schema_upgrades_to_m1_without_data_loss() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("connect to throwaway postgres");

    // Reset to a clean slate: this test owns the whole database.
    sqlx::query("drop schema public cascade")
        .execute(&pool)
        .await
        .expect("drop schema");
    sqlx::query("create schema public")
        .execute(&pool)
        .await
        .expect("recreate schema");

    // Step 1: apply the M0 schema only (migrations 0001-0003), as if this
    // were a database that already had M0 deployed to it.
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let all_migrations_dir = repo_root.join("migrations");

    let m0_dir = tempfile_m0_dir(&all_migrations_dir);
    Migrator::new(m0_dir.path())
        .await
        .expect("load M0 migrator")
        .run(&pool)
        .await
        .expect("apply M0 migrations");

    // Step 2: insert sample data into M0 tables.
    let user_id: uuid::Uuid =
        sqlx::query_scalar("insert into users (email, password_hash) values ($1, $2) returning id")
            .bind("m1-upgrade-test@example.com")
            .bind("not-a-real-hash")
            .fetch_one(&pool)
            .await
            .expect("insert sample user");

    sqlx::query("insert into agents (cert_fingerprint, cert_serial, hostname) values ($1, $2, $3)")
        .bind("aa:bb:cc")
        .bind("1")
        .bind("upgrade-test-host")
        .execute(&pool)
        .await
        .expect("insert sample agent");

    // Step 3: apply the full migration set, i.e. later milestone migrations
    // land on top of the already-deployed M0 schema.
    Migrator::new(all_migrations_dir)
        .await
        .expect("load full migrator")
        .run(&pool)
        .await
        .expect("apply M1 migrations on top of M0 schema");

    // Step 4: the M0 data must be exactly as inserted -- no loss, no
    // mutation.
    let email: String = sqlx::query_scalar("select email from users where id = $1")
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .expect("sample user still present");
    assert_eq!(email, "m1-upgrade-test@example.com");

    let agent_count: i64 =
        sqlx::query_scalar("select count(*) from agents where hostname = 'upgrade-test-host'")
            .fetch_one(&pool)
            .await
            .expect("sample agent still present");
    assert_eq!(agent_count, 1);

    // And the new M1 tables must exist and be empty (schema landed, no
    // data corruption).
    let device_count: i64 = sqlx::query_scalar("select count(*) from devices")
        .fetch_one(&pool)
        .await
        .expect("devices table exists after upgrade");
    assert_eq!(device_count, 0);

    let review_count: i64 = sqlx::query_scalar("select count(*) from service_review_items")
        .fetch_one(&pool)
        .await
        .expect("service review table exists after upgrade");
    assert_eq!(review_count, 0);

    let proposal_count: i64 = sqlx::query_scalar("select count(*) from monitor_proposals")
        .fetch_one(&pool)
        .await
        .expect("monitor proposal table exists after upgrade");
    assert_eq!(proposal_count, 0);

    let monitor_count: i64 = sqlx::query_scalar("select count(*) from monitors")
        .fetch_one(&pool)
        .await
        .expect("monitor table exists after upgrade");
    assert_eq!(monitor_count, 0);

    let result_count: i64 = sqlx::query_scalar("select count(*) from monitor_results")
        .fetch_one(&pool)
        .await
        .expect("monitor result table exists after upgrade");
    assert_eq!(result_count, 0);

    let incident_count: i64 = sqlx::query_scalar("select count(*) from incidents")
        .fetch_one(&pool)
        .await
        .expect("incident table exists after upgrade");
    assert_eq!(incident_count, 0);

    let rollup_count: i64 = sqlx::query_scalar("select count(*) from monitor_result_rollups")
        .fetch_one(&pool)
        .await
        .expect("monitor result rollup table exists after upgrade");
    assert_eq!(rollup_count, 0);

    let rolled_up_column: String = sqlx::query_scalar(
        "select data_type from information_schema.columns \
         where table_schema = 'public' and table_name = 'monitor_results' \
           and column_name = 'rolled_up_at'",
    )
    .fetch_one(&pool)
    .await
    .expect("monitor result rollup marker exists");
    assert_eq!(rolled_up_column, "timestamp with time zone");

    let interval_default: String = sqlx::query_scalar(
        "select column_default from information_schema.columns \
         where table_schema = 'public' and table_name = 'monitors' \
           and column_name = 'interval_seconds'",
    )
    .fetch_one(&pool)
    .await
    .expect("monitor interval default exists");
    assert_eq!(interval_default, "30");

    let failure_default: String = sqlx::query_scalar(
        "select column_default from information_schema.columns \
         where table_schema = 'public' and table_name = 'monitors' \
           and column_name = 'failure_threshold'",
    )
    .fetch_one(&pool)
    .await
    .expect("monitor failure default exists");
    assert_eq!(failure_default, "3");

    let underlying_default: String = sqlx::query_scalar(
        "select column_default from information_schema.columns \
         where table_schema = 'public' and table_name = 'monitors' \
           and column_name = 'underlying_state'",
    )
    .fetch_one(&pool)
    .await
    .expect("monitor underlying state default exists");
    assert_eq!(underlying_default, "'unknown'::text");
}

/// Build a temp directory containing only the M0 migration files
/// (0001-0003), so `Migrator` sees the M0-only schema.
fn tempfile_m0_dir(migrations_dir: &std::path::Path) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("create temp migrations dir");
    for name in ["0001_init.sql", "0002_agents.sql", "0003_sessions.sql"] {
        std::fs::copy(migrations_dir.join(name), dir.path().join(name))
            .unwrap_or_else(|e| panic!("copy {name}: {e}"));
    }
    dir
}
