use argon2::{Argon2, PasswordHasher};
use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::PasswordVerifier;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use std::time::{SystemTime, UNIX_EPOCH};
use pgvector::Vector;
use sqlx::Row;

use crate::models::*;
use crate::embeddings::*;

/* user information */

// route for user registration
pub(crate) async fn register_user(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Json(user_information): axum::extract::Json<RegisterUser>
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {


    // we use argon2 for password hashing

    // create a random salt
    let salt = SaltString::generate(&mut OsRng);

    // now hash the password
    let password_hash = Argon2::default()
        .hash_password(user_information.password.as_bytes(), &salt)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .to_string();

    // now, we insert the user into the database
    sqlx::query!("INSERT into users (username, email, password_hash)
        VALUES ($1, $2, $3)",  
        user_information.username,
        user_information.email,
        password_hash
    )
    .execute(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(axum::http::StatusCode::CREATED)
}

// route for user login (verifying credentials)
pub(crate) async fn user_login(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Json(login_information): axum::extract::Json<LoginUser>
) -> Result<axum::Json<LoginResponse>, (axum::http::StatusCode, String)> {
    // fetch the user from the database by username or email

    let user_record = sqlx::query!("SELECT id, password_hash FROM users WHERE username = $1 OR email = $2",
        login_information.identifier,
        login_information.identifier
    )
        .fetch_one(&state.pool)
        .await
        .map_err(|_e| (axum::http::StatusCode::UNAUTHORIZED, "Invalid username/email or password".to_string()))?;

    // verify the password
    let parsed_hash = argon2::PasswordHash::new(&user_record.password_hash)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Argon2::default()
        .verify_password(login_information.password.as_bytes(), &parsed_hash)
        .map_err(|_| (axum::http::StatusCode::UNAUTHORIZED, "Invalid username/email or password".to_string()))?;


    // jwt generation

    // get the current time and compute the expiration time
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let exp = now + (JWT_EXPIRATION_HOURS as u64 * 3600); // convert hours to seconds

    // create a claim for the user ID and expiration time
    let claims = Claims {
        sub: user_record.id.to_string(), // convert UUID to string for the JWT claim
        exp: exp as usize, // expiration time as a unix timestamp
    };

    // set our algorithm to HS256 (defaults to this regardless, but we set it explicitly for clarity)
    let header = Header::new(Algorithm::HS256);

    // get our secret key as an encoding key
    let encoding_key = EncodingKey::from_secret(state.jwt_secret.as_bytes()); // convert the secret string to bytes for the encoding key

    // encode the JWT
    let token = jsonwebtoken::encode(&header, &claims, &encoding_key)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    
    // make the response struct with the user ID and access token
    let response = axum::Json(LoginResponse {
        user_id: user_record.id,
        access_token: token, 
    });

    Ok(response)
}


/* transactions */

// route for adding a transaction
pub(crate) async fn add_transaction(
    auth: AuthenticatedUser,
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Json(req): axum::extract::Json<AddTransactionRequest>
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {

    // convert the TransactionKind to a string for storage
    let transaction_type = match req.kind {
        TransactionKind::Income => "income",
        TransactionKind::Expense => "expense",
    };

    // insert the transaction into the database
    let inserted_transaction = sqlx::query!("INSERT into transactions (user_id, amount, kind, category, date, description)
        VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
        auth.user_id,
        req.amount,
        transaction_type,
        req.category,
        req.date,
        req.description
    )
    .fetch_one(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let transaction_id = inserted_transaction.id;

    // now we call our embedding generation function to generate an embedding for this transaction
    let embedding_text = req.transaction_string_embedding();

    let embedding = generate_transaction_embedding(&state, &embedding_text).await?;

    // store the embedding in the database linked to this transaction
    store_transaction_embedding(&state, transaction_id, auth.user_id, &embedding_text, embedding).await?;

    Ok(axum::http::StatusCode::CREATED)
}


// route for getting transactions for authenticated user
pub(crate) async fn get_transactions(
    auth: AuthenticatedUser,
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<axum::Json<Vec<Transaction>>, (axum::http::StatusCode, String)> {

    // fetch all the user's transactions from the database
    let transactions = sqlx::query!(
        "SELECT amount, kind, category, date, description FROM transactions WHERE user_id = $1",
        auth.user_id
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // map the transactions from the database into Transaction structs
    let result: Vec<Transaction> = transactions
        .into_iter()
        .map(|transaction| Transaction {
            user_id: auth.user_id,
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
pub(crate) async fn upsert_budget(
    auth: AuthenticatedUser,
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Json(req): axum::extract::Json<UpsertBudgetRequest>
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {

    // insert the budget into the database (or update if it already exists)
    sqlx::query!(
        "INSERT INTO budgets (user_id, month, category, amount)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (user_id, month, category)
         DO UPDATE SET amount = EXCLUDED.amount, updated_at = CURRENT_TIMESTAMP",
        auth.user_id,
        req.month,
        req.category,
        req.amount
    )
    .execute(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(axum::http::StatusCode::CREATED)
}


// route for getting budgets for authenticated user (optionally filtered by month)
pub(crate) async fn get_budgets(
    auth: AuthenticatedUser,
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
            auth.user_id,
            month
        )
        .fetch_all(&state.pool)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        rows.into_iter()
            .map(|row| Budget {
                user_id: auth.user_id,
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
            auth.user_id
        )
        .fetch_all(&state.pool)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        rows.into_iter()
            .map(|row| Budget {
                user_id: auth.user_id,
                month: row.month,
                category: row.category,
                amount: row.amount,
            })
            .collect()
    };

    Ok(axum::Json(result))
}



// route for getting budget progress for authenticated user (budget vs spent) for a month
pub(crate) async fn get_budget_progress(
    auth: AuthenticatedUser,
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
        auth.user_id,
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



// route for semantically searching transactions by embedding similarity
pub(crate) async fn semantic_transaction_search(
    auth: AuthenticatedUser,
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Json(req): axum::extract::Json<SemanticSearchRequest>,
) -> Result<axum::Json<Vec<Transaction>>, (axum::http::StatusCode, String)> {

    // convert the search query into an embedding
    let search_embedding = generate_transaction_embedding(&state, &req.query).await?;

    // we extract the limit parameter from the query, defaulting to 10 if not provided and clamping at 50
    let amount = req.limit.unwrap_or(10).clamp(1, 50);

    // now we perform a similarity search in the database using the pgvector extension
    
    /* 
        We use cosine similarity for our search here for a number of reasons, such as
        direction being more important than magnitude for our use case, but also with
        our current model, embeddings are normalized to unit length anyways, so cosine
        similarity is essentially just a (slightly faster) dot product in our case, 
        which returns the same ranking as Euclidian distance anyways.
     */

     // in pgvector, we rank by cosine similarity using the <=> operator
     // this specifically calculates cosine distance, which is 1 - cosine similarity, so smaller values are more similar
     // therefore we order by this value ascending to get the most similar results first
    let rows = sqlx::query(
        "SELECT t.user_id, t.amount, t.kind, t.category, t.date, t.description
        FROM transaction_embeddings embed
        JOIN transactions t ON t.id = embed.transaction_id
        WHERE embed.user_id = $1
        ORDER BY embed.embedding <=> $2
        LIMIT $3"
    )
    .bind(auth.user_id)
    .bind(Vector::from(search_embedding))
    .bind(amount as i64)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let transactions: Vec<Transaction> = rows
        .into_iter()
        .map(|row| {
            let kind_str: String = row.get("kind");
            let kind = match kind_str.as_str() {
                "income" => TransactionKind::Income,
                "expense" => TransactionKind::Expense,
                _ => TransactionKind::Expense,
            };

            Transaction {
                user_id: row.get("user_id"),
                amount: row.get("amount"),
                kind,
                category: row.get("category"),
                date: row.get("date"),
                description: row.get("description"),
            }
        })
        .collect();

    Ok(axum::Json(transactions))
}


/* testing */

// test route
pub(crate) async fn test_handler() -> &'static str {
    "Test route is working!"
}

// test state access
pub(crate) async fn test_state_handler(
    axum::extract::State(_state): axum::extract::State<AppState>,
) -> &'static str {
    // we can access the database pool via state.pool
    "State access is working!"
}

// test database access
pub(crate) async fn test_db_handler(
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
