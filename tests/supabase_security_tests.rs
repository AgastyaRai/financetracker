mod common;

use financetracker::build_app;
use http_body_util::BodyExt;
use sqlx::{PgPool, migrate::Migrator, postgres::PgPoolOptions};
use tower::util::ServiceExt;

const MIGRATIONS: Migrator = sqlx::migrate!("./backend/migrations");
const SECURITY_SQL: &str = include_str!("../backend/deployment/supabase_security.sql");
const PRIVATE_TABLES: &[&str] = &[
    "_sqlx_migrations", "users", "transactions", "budgets", "sessions",
    "transaction_embeddings", "bank_connections", "imported_transaction_details",
];

async fn supabase_fixture(pool: &PgPool) -> PgPool {
    // These cluster roles are created only by tests running against the disposable test server.
    sqlx::raw_sql(
        "DO $$ BEGIN
            BEGIN CREATE ROLE anon NOLOGIN; EXCEPTION WHEN duplicate_object THEN NULL; END;
            BEGIN CREATE ROLE authenticated NOLOGIN; EXCEPTION WHEN duplicate_object THEN NULL; END;
        END $$;
        CREATE SCHEMA extensions;
        GRANT USAGE ON SCHEMA public, extensions TO anon, authenticated;
        GRANT ALL ON ALL TABLES IN SCHEMA public TO anon, authenticated;
        ALTER DEFAULT PRIVILEGES FOR ROLE postgres IN SCHEMA public
            GRANT ALL ON TABLES TO anon, authenticated;
        ALTER DEFAULT PRIVILEGES FOR ROLE postgres IN SCHEMA public
            GRANT ALL ON SEQUENCES TO anon, authenticated;
        CREATE TABLE public.shared_reference (label text);
        INSERT INTO public.shared_reference VALUES ('unrelated Data API table');"
    ).execute(pool).await.unwrap();

    // Match Supabase's existing backend search path, including on newly opened connections.
    PgPoolOptions::new()
        .max_connections(5)
        .after_connect(|connection, _| Box::pin(async move {
            sqlx::query("SET search_path = public, extensions")
                .execute(connection).await?;
            Ok(())
        }))
        .connect_with(pool.connect_options().as_ref().clone())
        .await.unwrap()
}

async fn apply_security(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::raw_sql(SECURITY_SQL).execute(&mut *transaction).await?;
    transaction.commit().await
}

async fn private_rows(pool: &PgPool) -> serde_json::Value {
    let mut rows = serde_json::Map::new();
    for table in PRIVATE_TABLES {
        let value: serde_json::Value = sqlx::query_scalar(&format!(
            "SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text), '[]'::jsonb) FROM public.{table} t"
        )).fetch_one(pool).await.unwrap();
        rows.insert(table.to_string(), value);
    }
    serde_json::Value::Object(rows)
}

async fn assert_private_permissions(pool: &PgPool) {
    for role in ["anon", "authenticated"] {
        for table in PRIVATE_TABLES {
            let allowed: bool = sqlx::query_scalar(
                "SELECT has_table_privilege($1::name, $2::text, 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')"
            ).bind(role).bind(format!("public.{table}")).fetch_one(pool).await.unwrap();
            assert!(!allowed, "{role} still has a privilege on {table}");

            let mut transaction = pool.begin().await.unwrap();
            sqlx::query(&format!("SET LOCAL ROLE {role}"))
                .execute(&mut *transaction).await.unwrap();
            let error = sqlx::query(&format!("SELECT * FROM public.{table}"))
                .execute(&mut *transaction).await.unwrap_err();
            assert_eq!(error.as_database_error().unwrap().code().as_deref(), Some("42501"));
            transaction.rollback().await.unwrap();
        }
        let unrelated_allowed: bool = sqlx::query_scalar(
            "SELECT has_table_privilege($1::name, 'public.shared_reference', 'SELECT')"
        ).bind(role).fetch_one(pool).await.unwrap();
        assert!(unrelated_allowed, "unrelated table grants must be preserved");
    }
}

#[sqlx::test(migrator = "MIGRATIONS")]
async fn security_setup_preserves_rows_and_backend_vector_search(pool: PgPool) {
    let pool = supabase_fixture(&pool).await;
    let mut state = common::setup_app_state().await;
    state.pool = pool.clone();
    let app = build_app(state);
    let (username, password) = common::create_and_register_test_user(&app).await;
    let (user_id, token) = common::login_test_user(&app, &username, &password).await;
    common::add_transaction(&app, &token, serde_json::json!({
        "amount": 25.50, "kind": "Expense", "date": "2026-10-13",
        "category": "Transportation", "description": "Uber ride home"
    })).await;
    let transaction_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM transactions WHERE user_id = $1"
    ).bind(user_id).fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO budgets (user_id, month, category, amount) VALUES ($1, '2026-10-01', 'Transportation', 200)")
        .bind(user_id).execute(&pool).await.unwrap();
    let connection_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO bank_connections (user_id, provider, provider_id, encrypted_access_token)
            VALUES ($1, 'fixture', 'security-test', decode('1234', 'hex')) RETURNING connection_id"
    ).bind(user_id).fetch_one(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO imported_transaction_details
            (transaction_id, connection_id, provider, provider_transaction_id, provider_account_id,
             source_status, original_payload, inferred_kind, inferred_category, inferred_description)
        VALUES ($1, $2, 'fixture', 'original-test', 'account-test', 'posted', '{\"original\":true}',
            'expense', 'Transportation', 'Original Uber ride home')"
    ).bind(transaction_id).bind(connection_id).execute(&pool).await.unwrap();
    let original_rows = private_rows(&pool).await;
    let vector_oid: i64 = sqlx::query_scalar("SELECT 'vector'::regtype::oid::bigint")
        .fetch_one(&pool).await.unwrap();
    let index_oid: i64 = sqlx::query_scalar("SELECT 'transaction_embeddings_embedding_idx'::regclass::oid::bigint")
        .fetch_one(&pool).await.unwrap();

    // Repeating provisioning must retain every record, the vector type, and the existing HNSW index.
    for _ in 0..2 {
        apply_security(&pool).await.unwrap();
        let schema: String = sqlx::query_scalar(
            "SELECT n.nspname FROM pg_extension e JOIN pg_namespace n ON n.oid=e.extnamespace WHERE e.extname='vector'"
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(schema, "extensions");
        assert_eq!(private_rows(&pool).await, original_rows);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT 'vector'::regtype::oid::bigint")
            .fetch_one(&pool).await.unwrap(), vector_oid);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT 'transaction_embeddings_embedding_idx'::regclass::oid::bigint")
            .fetch_one(&pool).await.unwrap(), index_oid);
        assert_private_permissions(&pool).await;
    }

    // Retire pooled connections to exercise SQLx's vector type lookup on a fresh connection too.
    let options = pool.connect_options().as_ref().clone();
    pool.close().await;
    let fresh_pool = PgPoolOptions::new()
        .after_connect(|connection, _| Box::pin(async move {
            sqlx::query("SET search_path = public, extensions").execute(connection).await?;
            Ok(())
        })).connect_with(options).await.unwrap();
    let mut state = common::setup_app_state().await;
    state.pool = fresh_pool.clone();
    let app = build_app(state);
    common::add_transaction(&app, &token, serde_json::json!({
        "amount": 10, "kind": "Expense", "date": "2026-10-13",
        "category": "Transportation", "description": "Uber ride to airport"
    })).await;
    let response = app.clone().oneshot(axum::http::Request::builder()
        .uri("/api/transactions").header("Authorization", format!("Bearer {token}"))
        .body(axum::body::Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &response.into_body().collect().await.unwrap().to_bytes()
    ).unwrap();
    assert_eq!(body.as_array().unwrap().len(), 2);
    assert!(body.as_array().unwrap().iter().any(|row| row["can_revert"] == true));

    let response = app.oneshot(axum::http::Request::builder()
        .method("POST").uri("/api/transactions/search/semantic")
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(r#"{"query":"uber","limit":10}"#)).unwrap()
    ).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &response.into_body().collect().await.unwrap().to_bytes()
    ).unwrap();
    assert_eq!(body["transactions"].as_array().unwrap().len(), 2);

    sqlx::raw_sql("CREATE TABLE public.future_private (id integer); CREATE SEQUENCE public.future_private_id_seq;")
        .execute(&fresh_pool).await.unwrap();
    for role in ["anon", "authenticated"] {
        let allowed: bool = sqlx::query_scalar(
            "SELECT has_table_privilege($1::name, 'public.future_private', 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')
                OR has_sequence_privilege($1::name, 'public.future_private_id_seq', 'USAGE,SELECT,UPDATE')"
        ).bind(role).fetch_one(&fresh_pool).await.unwrap();
        assert!(!allowed, "future private objects must not inherit any Data API grants");
    }
}

#[sqlx::test(migrator = "MIGRATIONS")]
async fn security_setup_rejects_existing_policies_without_partial_changes(pool: PgPool) {
    let pool = supabase_fixture(&pool).await;
    sqlx::raw_sql("ALTER TABLE users ENABLE ROW LEVEL SECURITY; CREATE POLICY existing_access ON users FOR SELECT TO anon USING (true);")
        .execute(&pool).await.unwrap();
    let error = apply_security(&pool).await.unwrap_err();
    assert!(error.to_string().contains("Existing Data API policies require review"));
    let allowed: bool = sqlx::query_scalar("SELECT has_table_privilege('anon', 'transactions', 'SELECT')")
        .fetch_one(&pool).await.unwrap();
    assert!(allowed, "a rejected setup must not revoke existing permissions");
    let schema: String = sqlx::query_scalar("SELECT n.nspname FROM pg_extension e JOIN pg_namespace n ON n.oid=e.extnamespace WHERE e.extname='vector'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(schema, "public");
}

#[sqlx::test(migrator = "MIGRATIONS")]
async fn security_setup_rejects_incompatible_backend_search_path(pool: PgPool) {
    let pool = supabase_fixture(&pool).await;
    let mut transaction = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL search_path = public").execute(&mut *transaction).await.unwrap();
    let error = sqlx::raw_sql(SECURITY_SQL).execute(&mut *transaction).await.unwrap_err();
    assert!(error.to_string().contains("Backend search_path must include extensions"));
    transaction.rollback().await.unwrap();
    let allowed: bool = sqlx::query_scalar("SELECT has_table_privilege('anon', 'transactions', 'SELECT')")
        .fetch_one(&pool).await.unwrap();
    assert!(allowed);
    let schema: String = sqlx::query_scalar("SELECT n.nspname FROM pg_extension e JOIN pg_namespace n ON n.oid=e.extnamespace WHERE e.extname='vector'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(schema, "public");
}


#[sqlx::test(migrations = false)]
async fn security_setup_supports_a_separately_managed_extension_owner(pool: PgPool) {
    // Supabase installs privileged extensions as its managed admin, not as the backend owner.
    sqlx::raw_sql(
        "DO $$ BEGIN
            BEGIN CREATE ROLE security_extension_owner NOLOGIN SUPERUSER;
                EXCEPTION WHEN duplicate_object THEN NULL; END;
        END $$;
        SET ROLE security_extension_owner;
        CREATE EXTENSION vector WITH SCHEMA public;
        RESET ROLE;"
    ).execute(&pool).await.unwrap();
    MIGRATIONS.run(&pool).await.unwrap();
    let pool = supabase_fixture(&pool).await;
    apply_security(&pool).await.unwrap();
    let (schema, owner): (String, String) = sqlx::query_as(
        "SELECT n.nspname, pg_get_userbyid(e.extowner)
            FROM pg_extension e JOIN pg_namespace n ON n.oid=e.extnamespace WHERE e.extname='vector'"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(schema, "extensions");
    assert_eq!(owner, "security_extension_owner");
    assert_private_permissions(&pool).await;
}
