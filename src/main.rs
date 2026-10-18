use sqlx::postgres::PgPoolOptions;

// import from our library crate
use financetracker::{AppState, build_app};
use financetracker::bank_credentials::BankCredentialCipher;
use financetracker::plaid::{PlaidClient, PlaidProvider, SANDBOX_BASE_URL};
use financetracker::embedding_worker::{EmbeddingWorkerConfig, run_embedding_worker, stop_embedding_worker};
use std::future::IntoFuture;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
 
async fn ensure_import_schema(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    let import_schema_ready = sqlx::query_scalar::<_, bool>(
        "SELECT to_regclass('bank_connections') IS NOT NULL
            AND to_regclass('imported_transaction_details') IS NOT NULL"
    )
    .fetch_one(pool)
    .await?;
    if !import_schema_ready {
        anyhow::bail!(
            "Required import tables are missing: the backend needs bank_connections and imported_transaction_details. Set RUN_MIGRATIONS=1 or run cargo sqlx migrate run --source backend/migrations before deploying."
        );
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    // load in env file
    dotenvy::from_filename("backend/.env").ok();

    // validate the optional encryption key before migrations or incoming requests can use it
    let bank_credential_cipher = BankCredentialCipher::from_env_value(
        std::env::var("BANK_CREDENTIAL_ENCRYPTION_KEY"),
    )
    .unwrap_or_else(|error| panic!("{error}"))
    .map(Arc::new);

    // set up the database connection
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    // set up the JWT secret key (for signing JWTs)
    let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");

    // set up the OpenAI API key (for generating embeddings)
    let openai_api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");

    // set up a reusable HTTP client for outbound API calls (like to OpenAI)
    let http_client = reqwest::Client::new();

    // select the Plaid environment and corresponding secret
    let (plaid_base_url, plaid_secret_var) = match std::env::var("PLAID_ENV").as_deref() {
        Ok("production") => ("https://production.plaid.com", "PLAID_PRODUCTION_SECRET"),
        Ok("sandbox") | Err(_) => (SANDBOX_BASE_URL, "PLAID_SANDBOX_SECRET"),
        _ => panic!("PLAID_ENV must be sandbox or production"),
    };

    // enable Sandbox linking only when credentials and country settings are supplied
    let plaid_provider: Option<Arc<dyn PlaidProvider>> = match (
        std::env::var("PLAID_CLIENT_ID"),
        std::env::var(plaid_secret_var),
        std::env::var("PLAID_COUNTRY_CODES"),
    ) {
        (Ok(client_id), Ok(secret), Ok(country_codes)) => {
            let country_codes: Vec<String> = country_codes.split(',')
                .map(|code| code.trim().to_string())
                .filter(|code| !code.is_empty())
                .collect();

            if client_id.trim().is_empty() || secret.trim().is_empty() || country_codes.is_empty() {
                None
            } else {
                // bound Plaid calls without changing the existing embedding client's behavior
                let plaid_http_client = reqwest::Client::builder()
                    .timeout(Duration::from_secs(10))
                    .build()
                    .expect("Could not create the Plaid HTTP client");
                Some(Arc::new(PlaidClient::new(
                    plaid_http_client,
                    plaid_base_url,
                    client_id,
                    secret,
                    country_codes,
                )))
            }
        }
        _ => None,
    };

    // debugging
    // print only host:port/path/query (everything after the last '@')
    if let Some(i) = db_url.rfind('@') {
        println!("DB target: {}", &db_url[i + 1..]);
    } else {
        println!("DB target: {}", db_url);
    }


    // create the connection pool, and connect lazily
    let pool = PgPoolOptions::new()
        .max_connections(15)
        .connect_lazy(&db_url)
        .expect("Could not create database connection pool");

    // run migrations when the environmental variable RUN_MIGRATIONS is set
    if std::env::var("RUN_MIGRATIONS").is_ok() {
        println!("Running database migrations...");
        sqlx::migrate!("./backend/migrations")
            .run(&pool)
            .await
            .expect("Could not run database migrations");
        println!("Database migrations complete.");
    }

    // Transaction reads depend on import provenance, so reject an incomplete schema before serving traffic.
    ensure_import_schema(&pool).await.expect("Database schema is not ready");

    // now, we set up the HTTP server so the frontend can call routes
    // we get the port number from the environment variable PORT, defaulting to 3000
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3000);

    // bind to 0.0.0.0 to accept connections from any IP
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));

    tracing::info!("Starting server on {}", addr);


    // set up the shared state
    let state = AppState { pool, jwt_secret, openai_api_key, http_client, embedding_provider: std::sync::Arc::new(financetracker::embeddings::OpenAIEmbeddingProvider), plaid_provider, bank_credential_cipher };

    // set up the router with the state
    let app = build_app(state.clone());

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    // Start exactly one worker after binding succeeds; it reconstructs unfinished work from the database.
    let worker_config = EmbeddingWorkerConfig::default();
    let pool = state.pool.clone();
    let (stop, mut shutdown) = watch::channel(false);
    let worker = tokio::spawn(run_embedding_worker(state, worker_config, shutdown.clone()));
    let server = axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(async move { let _ = shutdown.changed().await; })
        .into_future();
    tokio::pin!(server);

    // Signal both tasks together: HTTP requests drain while the worker finishes its current item.
    // Also stop and join the worker if serving fails before an operating-system shutdown signal arrives.
    let server_result = tokio::select! {
        result = &mut server => Some(result),
        _ = shutdown_signal() => None,
    };
    let _ = stop.send(true);
    let (server_result, worker_result) = tokio::join!(
        async {
            match server_result {
                Some(result) => result,
                None => server.await,
            }
        },
        stop_embedding_worker(worker, worker_config.shutdown_grace),
    );
    pool.close().await;
    worker_result.expect("Embedding worker failed during shutdown");
    server_result.expect("HTTP server failed");

    println!("Hello, world!");
}

// Ctrl+C works locally on Windows; deployed Unix processes also receive SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("Could not listen for Ctrl+C");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Could not listen for SIGTERM")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
