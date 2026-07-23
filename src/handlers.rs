use argon2::{Argon2, PasswordHasher};
use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::PasswordVerifier;
use axum::http::StatusCode;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use std::time::{SystemTime, UNIX_EPOCH};
use pgvector::Vector;
use sqlx::Row;

use crate::models::*;
use crate::embeddings::*;
use crate::ai::generate_semantic_search_summary;

/* constants */

// maximum number of results to return for semantic search
const MAX_SEARCH_RESULTS: i32 = 50; 
// maximum number of characters allowed in a semantic search query
const MAX_SEARCH_QUERY_LENGTH: usize = 500;
// minimum similarity (cosine distance) for search results, to filter out results that are completely irrelevant
const MAX_COSINE_DISTANCE: f32 = 0.70; // corresponds to a cosine similarity of 0.30

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

// semantic fields currently stored for a transaction
#[derive(sqlx::FromRow, PartialEq)]
struct TransactionSemanticFields {
    kind: String,
    category: Option<String>,
    description: Option<String>,
}

// borrowed semantic fields from a transaction request
// are copied into the same owned structure used for database results
impl TransactionSemanticFields {
    fn from_request(req: &AddTransactionRequest) -> Self {
        let kind = match req.kind {
            TransactionKind::Income => "income",
            TransactionKind::Expense => "expense",
        };

        Self {
            kind: kind.to_string(),
            category: req.category.clone(),
            description: req.description.clone(),
        }
    }
}

// enforce the transaction amount invariant at the API boundary before any database or provider work
fn validate_transaction_request(
    req: &AddTransactionRequest,
) -> Result<(), (StatusCode, String)> {
    if req.amount <= rust_decimal::Decimal::ZERO {
        return Err((
            StatusCode::BAD_REQUEST,
            "Amount must be greater than zero".to_string(),
        ));
    }

    Ok(())
}

// route for adding a transaction
pub(crate) async fn add_transaction(
    auth: AuthenticatedUser,
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Json(req): axum::extract::Json<AddTransactionRequest>
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {

    validate_transaction_request(&req)?;

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
    // store the embedding in the database linked to this transaction
    match TransactionEmbedding::generate_from_request(&state, &req).await {
        Ok(transaction_embedding) => {
            match store_transaction_embedding_if_current(
                &state,
                transaction_id,
                auth.user_id,
                transaction_embedding,
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => {
                    tracing::warn!(
                        transaction_id = %transaction_id,
                        user_id = %auth.user_id,
                        "transaction changed before its embedding was stored; stale embedding was skipped"
                    );
                }
                Err((status, error)) => {
                    tracing::warn!(
                        transaction_id = %transaction_id,
                        user_id = %auth.user_id,
                        status = %status,
                        error = %error,
                        "transaction created without a stored embedding; semantic search will retry it"
                    );
                }
            }
        }
        Err((status, error)) => {
            tracing::warn!(
                transaction_id = %transaction_id,
                user_id = %auth.user_id,
                status = %status,
                error = %error,
                "transaction created without an embedding; semantic search will retry it"
            );
        }
    }

    Ok(axum::http::StatusCode::CREATED)
}

// route for updating a transaction for the authenticated user
pub(crate) async fn update_transaction(
    auth: AuthenticatedUser,
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(transaction_id): axum::extract::Path<uuid::Uuid>,
    axum::extract::Json(req): axum::extract::Json<AddTransactionRequest>
) -> Result<StatusCode, (StatusCode, String)> {

    validate_transaction_request(&req)?;

    // convert the TransactionKind to a string for storage
    let requested_fields = TransactionSemanticFields::from_request(&req);

    let mut database_transaction = state.pool.begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // lock the transaction while checking ownership and whether its semantic fields changed
    let existing_fields = sqlx::query_as::<_, TransactionSemanticFields>(
        "SELECT kind, category, description
         FROM transactions
         WHERE id = $1 AND user_id = $2
         FOR UPDATE"
    )
    .bind(transaction_id)
    .bind(auth.user_id)
    .fetch_optional(&mut *database_transaction)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some(existing_fields) = existing_fields else {
        return Err((StatusCode::NOT_FOUND, "Transaction not found".to_string()));
    };

    let semantic_fields_changed = requested_fields != existing_fields;

    // update the financial data before making any external provider request
    sqlx::query(
        "UPDATE transactions
         SET amount = $1, kind = $2, category = $3, date = $4, description = $5
         WHERE id = $6 AND user_id = $7"
    )
    .bind(req.amount)
    .bind(&requested_fields.kind)
    .bind(requested_fields.category.as_deref())
    .bind(req.date)
    .bind(requested_fields.description.as_deref())
    .bind(transaction_id)
    .bind(auth.user_id)
    .execute(&mut *database_transaction)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // remove an outdated embedding in the same transaction as the semantic change
    if semantic_fields_changed {
        sqlx::query("DELETE FROM transaction_embeddings WHERE transaction_id = $1")
            .bind(transaction_id)
            .execute(&mut *database_transaction)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    database_transaction.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !semantic_fields_changed {
        return Ok(StatusCode::OK);
    }

    // generate the replacement embedding after the transaction has safely committed
    refresh_transaction_embedding(
        &state,
        transaction_id,
        auth.user_id,
        &requested_fields,
    )
    .await;

    Ok(StatusCode::OK)
}

// embedding generation is intentionally separate from the database transaction so a slow provider does not hold a row lock
async fn refresh_transaction_embedding(
    state: &AppState,
    transaction_id: uuid::Uuid,
    user_id: uuid::Uuid,
    semantic_fields: &TransactionSemanticFields,
) {
    // a provider failure leaves the embedding missing so semantic search can backfill it later
    let transaction_embedding = match TransactionEmbedding::generate(
        state,
        &semantic_fields.kind,
        semantic_fields.category.as_deref(),
        semantic_fields.description.as_deref(),
    )
    .await
    {
        Ok(transaction_embedding) => transaction_embedding,
        Err((status, error)) => {
            tracing::warn!(
                transaction_id = %transaction_id,
                user_id = %user_id,
                status = %status,
                error = %error,
                "transaction updated without an embedding; semantic search will retry it"
            );
            return;
        }
    };

    // the guarded store rejects this result if another update changed the semantic fields while the provider was running
    match store_transaction_embedding_if_current(
        state,
        transaction_id,
        user_id,
        transaction_embedding,
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(
                transaction_id = %transaction_id,
                user_id = %user_id,
                "transaction changed before its replacement embedding was stored; stale embedding was skipped"
            );
        }
        Err((status, error)) => {
            tracing::warn!(
                transaction_id = %transaction_id,
                user_id = %user_id,
                status = %status,
                error = %error,
                "transaction updated without a stored embedding; semantic search will retry it"
            );
        }
    }
}


// route for getting transactions for authenticated user
pub(crate) async fn get_transactions(
    auth: AuthenticatedUser,
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<axum::Json<Vec<Transaction>>, (axum::http::StatusCode, String)> {

    // fetch all the user's transactions from the database
    let transactions = sqlx::query!(
        "SELECT id, amount, kind, category, date, description FROM transactions WHERE user_id = $1",
        auth.user_id
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // map the transactions from the database into Transaction structs
    let result: Vec<Transaction> = transactions
        .into_iter()
        .map(|transaction| Transaction {
            id: transaction.id,
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
    // we left join transactions to  include categories with a budget but no expenses (spent = 0)
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
) -> Result<axum::Json<SemanticSearchResult>, (axum::http::StatusCode, String)> {

    let query = req.query.trim();

    if query.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Search query cannot be empty".to_string(),
        ));
    }

    if query.chars().count() > MAX_SEARCH_QUERY_LENGTH {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            format!("Search query cannot exceed {} characters", MAX_SEARCH_QUERY_LENGTH),
        ));
    }

    // we take this as an opportunity to perform a backfill of the users transactions who have no entry
    // in the transaction_embeddings table yet, so we generate and insert embeddings for any such
    // transactions
    let missing_rows = sqlx::query(
        "SELECT t.id, t.user_id, t.kind, t.category, t.description
         FROM transactions t
         WHERE t.user_id = $1
         AND NOT EXISTS (
            SELECT 1
            FROM transaction_embeddings e
            WHERE e.transaction_id = t.id
         )
         LIMIT 5"
    )
    .bind(auth.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // now we generate and insert embeddings for these transactions
    for row in missing_rows {
        let transaction_id: uuid::Uuid = row.get("id");
        let user_id: uuid::Uuid = row.get("user_id");
        let kind_str: String = row.get("kind");
        let category: Option<String> = row.get("category");
        let description: Option<String> = row.get("description");

        let transaction_embedding = TransactionEmbedding::generate(
            &state,
            &kind_str,
            category.as_deref(),
            description.as_deref(),
        )
        .await;

        // if a backfill row fails, we don't want the whole search to fail
        if let Ok(transaction_embedding) = transaction_embedding {
            let _ = store_transaction_embedding_if_current(
                &state,
                transaction_id,
                user_id,
                transaction_embedding,
            )
            .await;
        }
    }

    // convert the search query into an embedding
    let search_embedding = generate_transaction_embedding(&state, query).await?;

    // we extract the limit parameter from the query, defaulting to 10 if not provided and clamping at MAX_SEARCH_RESULTS
    let amount = req.limit.unwrap_or(10).clamp(1, MAX_SEARCH_RESULTS);

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
    
    // we additionally filter results by a maximum cosine distance threshold to avoid returning completely irrelevant results,
    // and we limit the number of results returned based on user input (defaulting to 10, max 50)
    let rows = sqlx::query(
        "SELECT t.id, t.user_id, t.amount, t.kind, t.category, t.date, t.description,
                    1.0 - (embed.embedding <=> $2) as similarity_score
        FROM transaction_embeddings embed
        JOIN transactions t ON t.id = embed.transaction_id
        WHERE embed.user_id = $1 AND embed.embedding <=> $2 < $3
        ORDER BY embed.embedding <=> $2
        LIMIT $4"
    )
    .bind(auth.user_id)
    .bind(Vector::from(search_embedding))
    .bind(MAX_COSINE_DISTANCE)
    .bind(amount as i64)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let semanticTransactions: Vec<SemanticTransaction> = rows
        .into_iter()
        .map(|row| {
            let kind_str: String = row.get("kind");
            let kind = match kind_str.as_str() {
                "income" => TransactionKind::Income,
                "expense" => TransactionKind::Expense,
                _ => TransactionKind::Expense,
            };


            let transaction = Transaction {
                id: row.get("id"),
                user_id: row.get("user_id"),
                amount: row.get("amount"),
                kind,
                category: row.get("category"),
                date: row.get("date"),
                description: row.get("description"),
            };

            SemanticTransaction {
                transaction,
                similarity_score: row.get("similarity_score"),
            }
        })
        .collect();

    // OPTIONAL: depending on the user's preference, we also choose to generate an AI summary
    // of the search results using the OpenAI API, returning this summary along with
    // the search results.
    
    if !req.summary.unwrap_or(false) {
        
        // if the user didn't request a summary, we just return the search results with no summary
        let result = SemanticSearchResult {
            transactions: semanticTransactions,
            summary: None,
        };

        return Ok(axum::Json(result));
    }

    let transactions : Vec<Transaction> = semanticTransactions.iter().map(|st| st.transaction.clone()).collect();

    // the user wants a summary. we'll be using the 4o-mini model for this, mainly because of the price,
    // and because we don't need a very long context window for this summary, since it's just
    // looking at search results (which we know will be at most 50 anyways) 
    let summary = generate_semantic_search_summary(&state, query, &transactions).await?;

    let result: SemanticSearchResult = SemanticSearchResult {
        transactions: semanticTransactions,
        summary: Some(summary),
    };

    Ok(axum::Json(result))
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
