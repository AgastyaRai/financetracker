use argon2::{Argon2, PasswordHasher};
use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::PasswordHash;
use argon2::PasswordVerifier;
use sqlx::types::Decimal;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::layer;
use tower_http::services::{ServeDir, ServeFile};

/* data structures */

// struct to hold shared application state
#[derive(Clone)]
    struct AppState {
        // so far, we only need the database pool
        pool: sqlx::PgPool,
    }

// struct for user registration
#[derive(serde::Deserialize)]
struct RegisterUser {
    username: String,
    email: String,
    password: String,
}

// structs for user login
#[derive(serde::Deserialize)]
struct LoginUser {
    identifier: String, // can be username or email
    password: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct LoginResponse {
    user_id: uuid::Uuid,
}

// enum for transaction kind
#[derive(serde::Deserialize, serde::Serialize)]
enum TransactionKind {
    Income,
    Expense,
}

// struct for adding a transaction
#[derive(serde::Deserialize, serde::Serialize)]
struct Transaction {
    user_id: uuid::Uuid,
    amount: Decimal,
    kind: TransactionKind,
    category: Option<String>,
    date: chrono::NaiveDate,
    description: Option<String>,
}

// struct for adding a budget
#[derive(serde::Deserialize, serde::Serialize)]
struct Budget {
    user_id: uuid::Uuid,
    month: chrono::NaiveDate, // first day of month (e.g., 2026-01-01)
    category: String,
    amount: Decimal,
}

// query params for budgets (optional month filter)
#[derive(serde::Deserialize)]
struct BudgetQuery {
    month: Option<chrono::NaiveDate>,
}

// struct for returning budget progress (budget vs spent)
#[derive(serde::Serialize)]
struct BudgetProgress {
    category: String,
    budget_amount: Decimal,
    spent: Decimal,
    remaining: Decimal,
}



#[tokio::main]
async fn main() {
    // load in env file
    dotenvy::dotenv_override().ok();

    // set up the database connection
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");

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
    let state = AppState { pool };

    use tower_http::cors::{CorsLayer, Any};

    // add a cors layer to allow requests from any origin (for development purposes)
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);


    // now, we set up our router

    // set up the api routes separately
    let api = axum::Router::new()
        // testing routes
        .route("/test", axum::routing::get(test_handler))
        .route("/test_state", axum::routing::get(test_state_handler))
        .route("/test_db", axum::routing::get(test_db_handler))

        // user routes
        .route("/users/register", axum::routing::post(register_user))
        .route("/users/login", axum::routing::post(user_login))

        // transaction routes
        .route("/transactions", axum::routing::post(add_transaction))
        .route("/transactions/:user_id", axum::routing::get(get_transactions))

        // budget routes
        .route("/budgets", axum::routing::post(upsert_budget))
        .route("/budgets/:user_id", axum::routing::get(get_budgets))
        .route("/budgets/:user_id/progress", axum::routing::get(get_budget_progress))

        // layer with CORS for development
        .layer(cors)
        .with_state(state);

    // we nest the api under /api 
    let app = axum::Router::new()
        .nest("/api", api)
        // serve the frontend static files from ./frontend/dist
        .fallback_service(
            ServeDir::new("./frontend/dist")
                .fallback(ServeFile::new("./frontend/dist/index.html")),
        );

    
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app.into_make_service()).await.unwrap();

    println!("Hello, world!");
}



// CRUD functions


/* user information */

// route for user registration
async fn register_user(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Json(userInformation): axum::extract::Json<RegisterUser>
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {


    // we use argon2 for password hashing

    // create a random salt
    let salt = SaltString::generate(&mut OsRng);

    // now hash the password
    let password_hash = Argon2::default()
        .hash_password(userInformation.password.as_bytes(), &salt)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .to_string();

    // now, we insert the user into the database
    sqlx::query!("INSERT into users (username, email, password_hash)
        VALUES ($1, $2, $3)",  
        userInformation.username,
        userInformation.email,
        password_hash
    )
    .execute(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(axum::http::StatusCode::CREATED)
}

// route for user login (verifying credentials)
async fn user_login(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Json(loginInformation): axum::extract::Json<LoginUser>
) -> Result<axum::Json<LoginResponse>, (axum::http::StatusCode, String)> {
    // fetch the user from the database by username or email

    let user_record = sqlx::query!("SELECT id, password_hash FROM users WHERE username = $1 OR email = $2",
        loginInformation.identifier,
        loginInformation.identifier
    )
        .fetch_one(&state.pool)
        .await
        .map_err(|e| (axum::http::StatusCode::UNAUTHORIZED, "Invalid username/email or password".to_string()))?;

    // verify the password
    let parsed_hash = argon2::PasswordHash::new(&user_record.password_hash)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Argon2::default()
        .verify_password(loginInformation.password.as_bytes(), &parsed_hash)
        .map_err(|_| (axum::http::StatusCode::UNAUTHORIZED, "Invalid username/email or password".to_string()))?;

    // make the response
    let response = axum::Json(LoginResponse {
        user_id: user_record.id,
    });

    Ok(response)
}


/* transactions */

// route for adding a transaction
async fn add_transaction(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Json(transaction): axum::extract::Json<Transaction>
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {

    // convert the TransactionKind to a string for storage
    let transaction_type = match transaction.kind {
        TransactionKind::Income => "income",
        TransactionKind::Expense => "expense",
    };

    // insert the transaction into the database
    sqlx::query!("INSERT into transactions (user_id, amount, kind, category, date, description)
        VALUES ($1, $2, $3, $4, $5, $6)",
        transaction.user_id,
        transaction.amount,
        transaction_type,
        transaction.category,
        transaction.date,
        transaction.description
    )
    .execute(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(axum::http::StatusCode::CREATED)
}


// route for getting transactions for a user
async fn get_transactions(
    axum::extract::Path(user_id): axum::extract::Path<uuid::Uuid>,
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<axum::Json<Vec<Transaction>>, (axum::http::StatusCode, String)> {

    // fetch all the users transactions from the database
    let transactions = sqlx::query!(
        "SELECT amount, kind, category, date, description FROM transactions WHERE user_id = $1",
        user_id
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // map the transactions from the database into Transaction structs
    let result: Vec<Transaction> = transactions
        .into_iter()
        .map(|transaction| Transaction {
            user_id: user_id,
            amount: transaction.amount,
            kind: match transaction.kind.as_str() {
                "income" => TransactionKind::Income,
                "expense" => TransactionKind::Expense,
                _ => panic!("Invalid transaction kind in database"),
            },
            category: transaction.category,
            date: transaction.date,
            description: transaction.description,   
        })
        .collect();

    Ok(axum::Json(result))
}

/* budgets */

// route for creating/updating a budget (upsert)
async fn upsert_budget(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Json(budget): axum::extract::Json<Budget>
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {

    // insert the budget into the database (or update if it already exists)
    sqlx::query!(
        "INSERT INTO budgets (user_id, month, category, amount)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (user_id, month, category)
         DO UPDATE SET amount = EXCLUDED.amount, updated_at = CURRENT_TIMESTAMP",
        budget.user_id,
        budget.month,
        budget.category,
        budget.amount
    )
    .execute(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(axum::http::StatusCode::CREATED)
}


// route for getting budgets for a user (optionally filtered by month)
async fn get_budgets(
    axum::extract::Path(user_id): axum::extract::Path<uuid::Uuid>,
    axum::extract::Query(query): axum::extract::Query<BudgetQuery>,
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<axum::Json<Vec<Budget>>, (axum::http::StatusCode, String)> {

    let result: Vec<Budget> = if let Some(month) = query.month {
        // fetch budgets for a specific month
        let rows = sqlx::query!(
            "SELECT month, category, amount
             FROM budgets
             WHERE user_id = $1 AND month = $2
             ORDER BY category ASC",
            user_id,
            month
        )
        .fetch_all(&state.pool)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        rows.into_iter()
            .map(|row| Budget {
                user_id,
                month: row.month,
                category: row.category,
                amount: row.amount,
            })
            .collect()
    } else {
        // fetch all budgets for the user
        let rows = sqlx::query!(
            "SELECT month, category, amount
             FROM budgets
             WHERE user_id = $1
             ORDER BY month DESC, category ASC",
            user_id
        )
        .fetch_all(&state.pool)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        rows.into_iter()
            .map(|row| Budget {
                user_id,
                month: row.month,
                category: row.category,
                amount: row.amount,
            })
            .collect()
    };

    Ok(axum::Json(result))
}



// route for getting budget progress for a user (budget vs spent) for a month
async fn get_budget_progress(
    axum::extract::Path(user_id): axum::extract::Path<uuid::Uuid>,
    axum::extract::Query(query): axum::extract::Query<BudgetQuery>,
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<axum::Json<Vec<BudgetProgress>>, (axum::http::StatusCode, String)> {

    use chrono::Datelike;

    // default to current month if not provided
    let month_start = if let Some(m) = query.month {
        m
    } else {
        let today = chrono::Utc::now().date_naive();
        chrono::NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap()
    };

    // compute next month start (exclusive end bound)
    let (ny, nm) = if month_start.month() == 12 {
        (month_start.year() + 1, 1)
    } else {
        (month_start.year(), month_start.month() + 1)
    };
    let next_month_start = chrono::NaiveDate::from_ymd_opt(ny, nm, 1).unwrap();

    // join budgets with transactions to compute "spent" per category (expenses only)
    let rows = sqlx::query!(
        "SELECT
            b.category as \"category!\",
            b.amount as \"budget_amount!\",
            COALESCE(SUM(t.amount), 0)::numeric as \"spent!\"
        FROM budgets b
        LEFT JOIN transactions t
        ON t.user_id = b.user_id
        AND t.kind = 'expense'
        AND t.category = b.category
        AND t.date >= $2
        AND t.date < $3
        WHERE b.user_id = $1
        AND b.month = $2
        GROUP BY b.category, b.amount
        ORDER BY b.category ASC",
        user_id,
        month_start,
        next_month_start
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;


    let result: Vec<BudgetProgress> = rows
        .into_iter()
        .map(|row| {
            let remaining = row.budget_amount - row.spent;
            BudgetProgress {
                category: row.category,
                budget_amount: row.budget_amount,
                spent: row.spent,
                remaining,
            }
        })
        .collect();

    Ok(axum::Json(result))
}


/* testing */

// test route
async fn test_handler() -> &'static str {
    "Test route is working!"
}

// test state access
async fn test_state_handler(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> &'static str {
    // we can access the database pool via state.pool
    "State access is working!"
}

// test database access
async fn test_db_handler(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<&'static str, (axum::http::StatusCode, String)> {
    
    // try a simple query to test database access
    sqlx::query!("SELECT 1 as one")
        .fetch_one(&state.pool)
        .await
        .map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database query failed: {}", e),
            )
        })?;

    Ok("Database access is working!")
}