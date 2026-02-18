use sqlx::postgres::PgPoolOptions;

// import from our library crate
use financetracker::{AppState, build_app};

#[tokio::main]
async fn main() {
    // load in env file
    dotenvy::dotenv_override().ok();

    // set up the database connection
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    // set up the JWT secret key (for signing JWTs)
    let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");

    // debugging
    // print only host:port/path/query (everything after the last '@')
    if let Some(i) = db_url.rfind('@') {
        println!("DB target: {}", &db_url[i + 1..]);
    } else {
        println!("DB target: {}", db_url);
    }


    // create the connection pool, and connect lazily
    let pool = PgPoolOptions::new()
        .max_connections(5)
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
    let state = AppState { pool, jwt_secret };

    // set up the router with the state
    let app = build_app(state);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app.into_make_service()).await.unwrap();

    println!("Hello, world!");
}
