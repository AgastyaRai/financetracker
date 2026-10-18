use argon2::{Argon2, PasswordHasher};
use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::PasswordVerifier;
use axum::http::StatusCode;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use std::time::{SystemTime, UNIX_EPOCH};
use pgvector::Vector;
use sqlx::Row;

use crate::bank_credentials::CredentialContext;
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


/* bank connections */

// read saved connection metadata for the authenticated user without contacting Plaid
pub(crate) async fn get_bank_connections(
    user: AuthenticatedUser,
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<axum::Json<Vec<BankConnectionResponse>>, (StatusCode, String)> {
    // Select only public fields; the login token determines whose connections can be read.
    let connections = sqlx::query_as::<_, BankConnectionResponse>(
        "SELECT connection_id, provider, status FROM bank_connections WHERE user_id = $1",
    )
    .bind(user.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|_| (
        StatusCode::INTERNAL_SERVER_ERROR,
        "Could not load bank connections".to_string(),
    ))?;

    Ok(axum::Json(connections))
}

// authenticate before creating a short-lived token; a connection is saved only after exchange
pub(crate) async fn create_bank_link_token(
    axum::extract::State(state): axum::extract::State<AppState>,
    user: AuthenticatedUser,
) -> Result<axum::Json<crate::plaid::LinkToken>, (StatusCode, String)> {
    let provider = state.plaid_provider.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        "Bank linking is not configured".to_string(),
    ))?;

    // Plaid authentication errors describe our backend credentials, not the user's login
    let token = provider.create_link_token(user.user_id).await.map_err(|_| (
        StatusCode::BAD_GATEWAY,
        "Could not create a bank-linking token".to_string(),
    ))?;

    Ok(axum::Json(token))
}

// finish linking only after authentication, provider exchange, encryption, and a successful insert
pub(crate) async fn exchange_bank_token(
    user: AuthenticatedUser,
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::Json(req): axum::Json<ExchangeBankTokenRequest>,
) -> Result<(StatusCode, axum::Json<BankConnectionResponse>), (StatusCode, String)> {
    if req.public_token.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Public token must not be blank".to_string()));
    }

    // verify both prerequisites before consuming the one-use token returned by Link
    let provider = state.plaid_provider.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        "Bank linking is not configured".to_string(),
    ))?;
    let cipher = state.bank_credential_cipher.as_ref().ok_or_else(|| (
        StatusCode::SERVICE_UNAVAILABLE,
        "Bank credential encryption is not configured".to_string(),
    ))?;

    // complete the external call before touching the database; no transaction or lock spans Plaid
    let connection = provider.exchange_public_token(&req.public_token).await.map_err(|_| (
        StatusCode::BAD_GATEWAY,
        "Could not complete bank linking".to_string(),
    ))?;
    let context = CredentialContext {
        user_id: user.user_id,
        provider: "plaid",
        provider_id: &connection.provider_id,
    };
    let encrypted_access_token = cipher.encrypt(&connection.access_token, &context).map_err(|_| (
        StatusCode::INTERNAL_SERVER_ERROR,
        "Could not secure bank connection".to_string(),
    ))?;

    // exchange cannot be rolled back with the insert; report save failures without retrying the token
    let saved = save_bank_connection(&state.pool, &context, &encrypted_access_token).await
        .map_err(|error| {
            if error.as_database_error().is_some_and(|error| error.is_unique_violation()) {
                return (StatusCode::CONFLICT, "Bank connection already exists".to_string());
            }
            (StatusCode::INTERNAL_SERVER_ERROR, "Could not save bank connection".to_string())
        })?;
    Ok((StatusCode::CREATED, axum::Json(saved)))
}

// one insert uses the same owner/provider context as encryption; uniqueness protects existing rows
async fn save_bank_connection(
    pool: &sqlx::PgPool,
    context: &CredentialContext<'_>,
    encrypted_access_token: &[u8],
) -> Result<BankConnectionResponse, sqlx::Error> {
    sqlx::query_as::<_, BankConnectionResponse>(
        "INSERT INTO bank_connections (user_id, provider, provider_id, encrypted_access_token)
         VALUES ($1, $2, $3, $4)
         RETURNING connection_id, provider, status",
    )
    .bind(context.user_id)
    .bind(context.provider)
    .bind(context.provider_id)
    .bind(encrypted_access_token)
    .fetch_one(pool)
    .await
}


// import the authenticated user's saved connection; no Plaid credential is accepted from the frontend
pub(crate) async fn import_bank_transactions(
    user: AuthenticatedUser,
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(connection_id): axum::extract::Path<uuid::Uuid>,
) -> Result<axum::Json<crate::imports::ImportResponse>, (StatusCode, String)> {
    crate::imports::import_transactions(&state, user.user_id, connection_id).await.map(axum::Json)
}


/* transactions */

// semantic fields currently stored for a transaction
#[derive(sqlx::FromRow, PartialEq)]
struct TransactionSemanticFields {
    kind: String,
    category: Option<String>,
    description: Option<String>,
}

// fields read under the transaction lock before accepting an edit
// row_version covers all saved changes, while semantic_fields determine whether indexing is needed
#[derive(sqlx::FromRow)]
struct TransactionEditState {
    #[sqlx(flatten)]
    semantic_fields: TransactionSemanticFields,
    row_version: i64,
    status: String,
}

// borrowed semantic fields from a transaction request
// are copied into the same owned structure used for database results
impl TransactionSemanticFields {
    fn from_request(req: &AddTransactionRequest) -> Self {
        let kind = match req.kind {
            TransactionKind::Income => "income",
            TransactionKind::Expense => "expense",
            TransactionKind::Transfer => "transfer",
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
    if req.amount == rust_decimal::Decimal::ZERO {
        return Err((
            StatusCode::BAD_REQUEST,
            "Amount must not be zero".to_string(),
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
        TransactionKind::Transfer => "transfer",
    };

    // insert the transaction into the database
    let inserted_transaction = sqlx::query!("INSERT into transactions (user_id, amount, kind, category, date, description)
        VALUES ($1, $2, $3, $4, $5, $6) RETURNING id, search_revision",
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
    let search_revision = inserted_transaction.search_revision;

    // now we call our embedding generation function to generate an embedding for this transaction
    // store the embedding in the database linked to this transaction
    match TransactionEmbedding::generate_from_request(&state, &req, search_revision).await {
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
                        "transaction created without a stored embedding; the background worker will retry it"
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
                "transaction created without an embedding; the background worker will retry it"
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
    axum::extract::Json(update): axum::extract::Json<UpdateTransactionRequest>
) -> Result<StatusCode, (StatusCode, String)> {

    let req = update.transaction;
    validate_transaction_request(&req)?;

    // convert the TransactionKind to a string for storage
    let requested_fields = TransactionSemanticFields::from_request(&req);

    let mut database_transaction = state.pool.begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // lock the transaction while checking ownership and whether its semantic fields changed
    let existing_transaction = sqlx::query_as::<_, TransactionEditState>(
        "SELECT kind, category, description, row_version, status
         FROM transactions
         WHERE id = $1 AND user_id = $2
         FOR UPDATE"
    )
    .bind(transaction_id)
    .bind(auth.user_id)
    .fetch_optional(&mut *database_transaction)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some(existing_transaction) = existing_transaction else {
        return Err((StatusCode::NOT_FOUND, "Transaction not found".to_string()));
    };

    // check the draft version under the same lock used by imports and other edits.
    // reject before changing financial data, override flags, or embeddings.
    if update.expected_version != existing_transaction.row_version || existing_transaction.status == "removed" {
        return Err((StatusCode::CONFLICT, "Transaction changed. Reload latest before saving.".to_string()));
    }

    let semantic_fields_changed = requested_fields != existing_transaction.semantic_fields;

    // compare each imported field with its stored value and preserve flags already set
    // update the financial data before making any external provider request
    let search_revision = sqlx::query_scalar::<_, i32>(
        "UPDATE transactions
         SET amount = $1, kind = $2, category = $3, date = $4, description = $5,
             kind_overridden = kind_overridden OR (origin = 'imported' AND kind IS DISTINCT FROM $2),
             category_overridden = category_overridden OR (origin = 'imported' AND category IS DISTINCT FROM $3),
             description_overridden = description_overridden OR (origin = 'imported' AND description IS DISTINCT FROM $5),
             row_version = row_version + 1,
             search_revision = search_revision + CASE WHEN $8 THEN 1 ELSE 0 END
         WHERE id = $6 AND user_id = $7
         RETURNING search_revision"
    )
    .bind(req.amount)
    .bind(&requested_fields.kind)
    .bind(requested_fields.category.as_deref())
    .bind(req.date)
    .bind(requested_fields.description.as_deref())
    .bind(transaction_id)
    .bind(auth.user_id)
    .bind(semantic_fields_changed)
    .fetch_one(&mut *database_transaction)
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
        search_revision,
    )
    .await;

    Ok(StatusCode::OK)
}

pub(crate) async fn revert_transaction(
    auth: AuthenticatedUser,
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(transaction_id): axum::extract::Path<uuid::Uuid>,
) -> Result<axum::Json<RevertedTransactionFields>, (StatusCode, String)> {
    let mut database_transaction = state.pool.begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // lock the owned bank connection before the transaction, matching the import's lock order.
    let connection_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT c.connection_id
         FROM bank_connections c
         JOIN imported_transaction_details d ON d.connection_id = c.connection_id
         JOIN transactions t ON t.id = d.transaction_id
         WHERE t.id = $1 AND t.user_id = $2 AND c.user_id = $2
         FOR SHARE OF c"
    )
    .bind(transaction_id)
    .bind(auth.user_id)
    .fetch_optional(&mut *database_transaction)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(connection_id) = connection_id else {
        return Err((StatusCode::NOT_FOUND, "Imported transaction not found".to_string()));
    };

    // Serialize restores with edits, while checking the transaction owner's identity.
    let existing_fields = sqlx::query_as::<_, TransactionSemanticFields>(
        "SELECT kind, category, description FROM transactions
         WHERE id = $1 AND user_id = $2 FOR UPDATE"
    )
    .bind(transaction_id)
    .bind(auth.user_id)
    .fetch_optional(&mut *database_transaction)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(existing_fields) = existing_fields else {
        return Err((StatusCode::NOT_FOUND, "Imported transaction not found".to_string()));
    };

    // A provenance row is usable only when its bank connection belongs to the same user.
    // Keep the source stable during the write without ever modifying its original payload.
    let original_fields = sqlx::query_as::<_, TransactionSemanticFields>(
        "SELECT d.inferred_kind AS kind, d.inferred_category AS category, d.inferred_description AS description
         FROM imported_transaction_details d
         JOIN bank_connections c ON c.connection_id = d.connection_id
         WHERE d.transaction_id = $1 AND c.user_id = $2 AND d.connection_id = $3
         FOR SHARE OF d, c"
    )
    .bind(transaction_id)
    .bind(auth.user_id)
    .bind(connection_id)
    .fetch_optional(&mut *database_transaction)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(original_fields) = original_fields else {
        return Err((StatusCode::NOT_FOUND, "Imported transaction not found".to_string()));
    };
    let kind = match original_fields.kind.as_str() {
        "income" => TransactionKind::Income,
        "expense" => TransactionKind::Expense,
        // The imported schema anticipates transfers; this branch's financial model does not yet support them.
        _ => return Err((StatusCode::CONFLICT, "The original transaction kind is not supported yet".to_string())),
    };
    let semantic_fields_changed = original_fields != existing_fields;

    // Only the three editable import classifications are restored; money and date stay current.
    // Release overrides even when the user has already typed the original values back in.
    let search_revision = sqlx::query_scalar::<_, i32>(
        "UPDATE transactions SET kind = $1, category = $2, description = $3,
         kind_overridden = FALSE, category_overridden = FALSE, description_overridden = FALSE,
         row_version = row_version + 1,
         search_revision = search_revision + CASE WHEN $6 THEN 1 ELSE 0 END
         WHERE id = $4 AND user_id = $5 RETURNING search_revision"
    )
    .bind(&original_fields.kind)
    .bind(original_fields.category.as_deref())
    .bind(original_fields.description.as_deref())
    .bind(transaction_id)
    .bind(auth.user_id)
    .bind(semantic_fields_changed)
    .fetch_one(&mut *database_transaction)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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

    // Provider failure cannot undo a restore; guarded storage also rejects delayed results after a newer edit.
    if semantic_fields_changed {
        refresh_transaction_embedding(&state, transaction_id, auth.user_id, &original_fields, search_revision).await;
    }

    Ok(axum::Json(RevertedTransactionFields {
        kind,
        category: original_fields.category,
        description: original_fields.description,
    }))
}

async fn revertible_transaction_ids(
    state: &AppState,
    user_id: uuid::Uuid,
    transaction_ids: &[uuid::Uuid],
) -> Result<std::collections::HashSet<uuid::Uuid>, (StatusCode, String)> {
    if transaction_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }

    let ids = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT t.id FROM transactions t
         JOIN imported_transaction_details d ON d.transaction_id = t.id
         JOIN bank_connections c ON c.connection_id = d.connection_id
         WHERE t.user_id = $1 AND c.user_id = $1 AND t.id = ANY($2)
           AND d.inferred_kind IN ('income', 'expense')
           AND (t.kind IS DISTINCT FROM d.inferred_kind
             OR t.category IS DISTINCT FROM d.inferred_category
             OR t.description IS DISTINCT FROM d.inferred_description
             OR t.kind_overridden OR t.category_overridden OR t.description_overridden)"
    )
    .bind(user_id)
    .bind(transaction_ids)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(ids.into_iter().collect())
}

// embedding generation is intentionally separate from the database transaction so a slow provider does not hold a row lock
async fn refresh_transaction_embedding(
    state: &AppState,
    transaction_id: uuid::Uuid,
    user_id: uuid::Uuid,
    semantic_fields: &TransactionSemanticFields,
    search_revision: i32,
) {
    // a provider failure leaves the embedding missing so the background worker can retry it later
    let transaction_embedding = match TransactionEmbedding::generate(
        state,
        &semantic_fields.kind,
        semantic_fields.category.as_deref(),
        semantic_fields.description.as_deref(),
        search_revision,
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
                "transaction updated without an embedding; the background worker will retry it"
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
                "transaction updated without a stored embedding; the background worker will retry it"
            );
        }
    }
}


// Report saved indexing progress without generating embeddings or changing transaction data.
pub(crate) async fn get_indexing_status(
    auth: AuthenticatedUser,
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<axum::Json<IndexingStatus>, (StatusCode, String)> {
    // One database snapshot keeps all three counts consistent while the worker saves new embeddings.
    // Match both owner and revision, just as the worker does when finding unfinished transactions.
    let status = sqlx::query_as::<_, IndexingStatus>(
        "SELECT COUNT(*) AS total,
                COUNT(e.transaction_id) AS indexed,
                COUNT(*) - COUNT(e.transaction_id) AS remaining
         FROM transactions t
         LEFT JOIN transaction_embeddings e
           ON e.transaction_id = t.id
          AND e.user_id = t.user_id
          AND e.search_revision = t.search_revision
         WHERE t.user_id = $1 AND t.status <> 'removed'",
    )
    .bind(auth.user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not load indexing status".to_string(),
        )
    })?;

    Ok(axum::Json(status))
}

// route for getting transactions for authenticated user
pub(crate) async fn get_transactions(
    auth: AuthenticatedUser,
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<axum::Json<Vec<Transaction>>, (axum::http::StatusCode, String)> {

    // fetch all the user's transactions from the database
    // keep removed transactions stored for history, but exclude them from this list
    let transactions = sqlx::query!(
        "SELECT id, amount, kind, category, date, description, row_version FROM transactions WHERE user_id = $1 AND status <> 'removed'",
        auth.user_id
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let transaction_ids: Vec<uuid::Uuid> = transactions.iter().map(|transaction| transaction.id).collect();
    let revertible_ids = revertible_transaction_ids(&state, auth.user_id, &transaction_ids).await?;

    // map the transactions from the database into Transaction structs
    let result: Vec<Transaction> = transactions
        .into_iter()
        .map(|transaction| Transaction {
            id: transaction.id,
            row_version: transaction.row_version,
            can_revert: revertible_ids.contains(&transaction.id),
            user_id: auth.user_id,
            amount: transaction.amount,
            kind: match transaction.kind.as_str() {
                "income" => TransactionKind::Income,
                "expense" => TransactionKind::Expense,
                "transfer" => TransactionKind::Transfer,
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
    // keep the status filter in the join so categories with only removed expenses still appear
    let rows = sqlx::query!(
        "SELECT
            b.category as \"category!\",
            b.amount as \"budget_amount!\",
            GREATEST(COALESCE(SUM(-t.amount), 0), 0)::numeric as \"spent!\"
        FROM budgets b
        LEFT JOIN transactions t
        ON t.user_id = b.user_id
        AND t.kind = 'expense'
        AND t.status <> 'removed'
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

    // Transaction indexing runs in the worker so backlog and retries cannot delay this request.
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
    // check the transaction owner too so an incorrect embedding user ID cannot expose another user's data
    // exclude removed transactions before ordering and limiting the search results
    let rows = sqlx::query(
        "SELECT t.id, t.user_id, t.amount, t.kind, t.category, t.date, t.description, t.row_version,
                    1.0 - (embed.embedding <=> $2) as similarity_score
        FROM transaction_embeddings embed
        JOIN transactions t
          ON t.id = embed.transaction_id
         AND t.search_revision = embed.search_revision
        WHERE embed.user_id = $1 AND t.user_id = $1 AND embed.embedding <=> $2 < $3
        AND t.status <> 'removed'
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

    let transaction_ids: Vec<uuid::Uuid> = rows.iter().map(|row| row.get("id")).collect();
    let revertible_ids = revertible_transaction_ids(&state, auth.user_id, &transaction_ids).await?;

    let semanticTransactions: Vec<SemanticTransaction> = rows
        .into_iter()
        .map(|row| {
            let kind_str: String = row.get("kind");
            let kind = match kind_str.as_str() {
                "income" => TransactionKind::Income,
                "expense" => TransactionKind::Expense,
                "transfer" => TransactionKind::Transfer,
                _ => TransactionKind::Expense,
            };


            let transaction = Transaction {
                id: row.get("id"),
                row_version: row.get("row_version"),
                can_revert: revertible_ids.contains(&row.get("id")),
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
