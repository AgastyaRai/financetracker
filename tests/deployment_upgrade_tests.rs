use sqlx::{ConnectOptions, PgPool, migrate::Migrator};
use std::{borrow::Cow, process::Stdio, time::Duration};
use tokio::process::Command;

const MIGRATIONS: Migrator = sqlx::migrate!("./backend/migrations");
const TEST_JWT_SECRET: &str = "deployment_upgrade_test_only";

async fn legacy_schema(pool: &PgPool) {
    // Model an existing deployment that applied the migrations before import provenance was added.
    let legacy_migrations = Migrator {
        migrations: Cow::Owned(
            MIGRATIONS
                .iter()
                .filter(|migration| migration.version <= 8)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    legacy_migrations.run(pool).await.unwrap();
}

fn backend(pool: &PgPool, run_migrations: bool, port: u16) -> Command {
    // SQLx 0.7 serializes Prefer as PREFERRED, which its URL parser does not accept; retain the same mode with its accepted spelling.
    let database_url = pool
        .connect_options()
        .to_url_lossy()
        .to_string()
        .replace("ssl-mode=PREFERRED", "ssl-mode=prefer");
    let mut command = Command::new(env!("CARGO_BIN_EXE_financetracker"));
    command
        .env("DATABASE_URL", database_url)
        .env("JWT_SECRET", TEST_JWT_SECRET)
        .env("OPENAI_API_KEY", "no_provider_requests_in_this_test")
        .env("PORT", port.to_string())
        .env_remove("RUN_MIGRATIONS")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if run_migrations {
        command.env("RUN_MIGRATIONS", "1");
    }
    command
}

fn available_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[sqlx::test(migrations = false)]
async fn startup_rejects_missing_import_schema_before_serving_requests(pool: PgPool) {
    legacy_schema(&pool).await;
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        backend(&pool, false, available_port()).output(),
    )
    .await
    .expect("an unmigrated backend must exit instead of serving broken transaction routes")
    .unwrap();

    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("bank_connections"),
        "missing table should be identified: {error}"
    );
    assert!(
        error.contains("imported_transaction_details"),
        "missing table should be identified: {error}"
    );
    assert!(
        error.contains("RUN_MIGRATIONS=1"),
        "startup should explain how to apply the upgrade: {error}"
    );
}

async fn financial_rows(pool: &PgPool, user_id: uuid::Uuid) -> serde_json::Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_object(
            'user', (SELECT to_jsonb(u) FROM users u WHERE id = $1),
            'transactions', (SELECT jsonb_agg(to_jsonb(t) ORDER BY t.id) FROM transactions t WHERE user_id = $1),
            'budgets', (SELECT jsonb_agg(to_jsonb(b) ORDER BY b.id) FROM budgets b WHERE user_id = $1))"
    ).bind(user_id).fetch_one(pool).await.unwrap()
}

#[sqlx::test(migrations = false)]
async fn startup_migrations_upgrade_existing_rows_and_are_safe_to_repeat(pool: PgPool) {
    legacy_schema(&pool).await;
    let user_id = uuid::Uuid::new_v4();
    let transaction_id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email, password_hash) VALUES ($1, 'upgrade-test', 'upgrade-test@example.com', 'unused-test-hash')")
        .bind(user_id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO transactions (id, user_id, amount, kind, category, description, date) VALUES ($1, $2, 25.50, 'expense', 'Groceries', 'Existing transaction', '2026-10-12')")
        .bind(transaction_id).bind(user_id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO budgets (user_id, month, category, amount) VALUES ($1, '2026-10-01', 'Groceries', 250.00)")
        .bind(user_id).execute(&pool).await.unwrap();
    let original_rows = financial_rows(&pool, user_id).await;
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &serde_json::json!({"sub": user_id.to_string(), "exp": chrono::Utc::now().timestamp() + 300}),
        &jsonwebtoken::EncodingKey::from_secret(TEST_JWT_SECRET.as_bytes()),
    ).unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();

    // Exercise the real binary twice: initial deployment upgrades the schema, and a restart must not reapply it.
    for _ in 0..2 {
        let port = available_port();
        let mut child = backend(&pool, true, port).spawn().unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let response = loop {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("backend exited before serving the upgraded schema: {status}");
            }
            if let Ok(response) = client
                .get(format!("http://127.0.0.1:{port}/api/transactions"))
                .bearer_auth(&token)
                .send()
                .await
            {
                break response;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "upgraded backend did not start"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let transactions: serde_json::Value = response.json().await.unwrap();
        assert_eq!(transactions.as_array().unwrap().len(), 1);
        assert_eq!(transactions[0]["id"], transaction_id.to_string());
        assert_eq!(transactions[0]["can_revert"], false);
        assert_eq!(financial_rows(&pool, user_id).await, original_rows);
        let applied: Vec<i64> = sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE version IN (9, 11) AND success ORDER BY version")
            .fetch_all(&pool).await.unwrap();
        assert_eq!(applied, vec![9, 11]);
        child.kill().await.unwrap();
        child.wait().await.unwrap();
    }
}
