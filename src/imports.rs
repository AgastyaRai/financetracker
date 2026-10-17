use axum::http::StatusCode;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use std::collections::HashSet;
use uuid::Uuid;

use crate::bank_credentials::CredentialContext;
use crate::models::AppState;
use crate::plaid::{PlaidError, PlaidProvider};

type ImportError = (StatusCode, String);

#[derive(serde::Serialize)]
pub(crate) struct ImportResponse {
    imported: usize,
}

#[derive(sqlx::FromRow, PartialEq)]
struct BankConnection {
    connection_id: Uuid,
    user_id: Uuid,
    provider: String,
    provider_id: String,
    encrypted_access_token: Option<Vec<u8>>,
    progress_cursor: Option<String>,
    status: String,
}

pub(crate) async fn import_transactions(
    state: &AppState,
    user_id: Uuid,
    connection_id: Uuid,
) -> Result<ImportResponse, ImportError> {
    // ownership is checked before decrypting a credential or making an external request
    let connection = sqlx::query_as::<_, BankConnection>(
        "SELECT connection_id, user_id, provider, provider_id, encrypted_access_token, progress_cursor, status
         FROM bank_connections WHERE connection_id = $1 AND user_id = $2",
    )
    .bind(connection_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(database_error)?
    .ok_or_else(|| (StatusCode::NOT_FOUND, "Bank connection not found".to_string()))?;
    if connection.status != "active" {
        return Err((
            StatusCode::CONFLICT,
            "Bank connection is not active".to_string(),
        ));
    }
    if connection.provider != "plaid" {
        return Err((
            StatusCode::CONFLICT,
            "Unsupported bank provider".to_string(),
        ));
    }
    let provider = state.plaid_provider.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Bank import is not configured".to_string(),
        )
    })?;
    let cipher = state.bank_credential_cipher.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Bank credential encryption is not configured".to_string(),
        )
    })?;
    let context = CredentialContext {
        user_id,
        provider: &connection.provider,
        provider_id: &connection.provider_id,
    };
    let encrypted = connection
        .encrypted_access_token
        .as_deref()
        .ok_or_else(credential_error)?;
    let access_token = cipher
        .decrypt(encrypted, &context)
        .map_err(|_| credential_error())?;

    // fetch and validate every page before taking database locks; failure leaves the saved cursor reusable
    let batch = match fetch_all_transactions(
        provider.as_ref(),
        &access_token,
        connection.progress_cursor.clone(),
    )
    .await
    {
        Ok(batch) => batch,
        Err(PlaidError::LoginRequired) => {
            mark_connection_needs_reconnection(&state.pool, &connection).await?;
            return Err((
                StatusCode::CONFLICT,
                "Bank connection needs reconnection".to_string(),
            ));
        }
        Err(PlaidError::RequestFailed) => return Err(provider_error()),
    };
    let imported = save_transactions(&state.pool, &connection, batch).await?;
    Ok(ImportResponse { imported })
}

async fn mark_connection_needs_reconnection(
    pool: &PgPool,
    connection: &BankConnection,
) -> Result<(), ImportError> {
    // a delayed failure must not change a connection whose credential, progress, or status has already changed
    let result = sqlx::query(
        "UPDATE bank_connections SET status = 'needs_reconnection', updated_at = CURRENT_TIMESTAMP
         WHERE connection_id = $1 AND user_id = $2 AND provider = $3 AND provider_id = $4
           AND encrypted_access_token = $5 AND progress_cursor IS NOT DISTINCT FROM $6 AND status = 'active'",
    )
    .bind(connection.connection_id)
    .bind(connection.user_id)
    .bind(&connection.provider)
    .bind(&connection.provider_id)
    .bind(&connection.encrypted_access_token)
    .bind(&connection.progress_cursor)
    .execute(pool)
    .await
    .map_err(database_error)?;
    if result.rows_affected() != 1 {
        return Err((
            StatusCode::CONFLICT,
            "Bank connection changed; retry the import".to_string(),
        ));
    }
    Ok(())
}

struct ImportBatch {
    transactions: Vec<BankTransaction>,
    removed: Vec<String>,
    next_cursor: String,
}

async fn fetch_all_transactions(
    provider: &dyn PlaidProvider,
    access_token: &str,
    mut cursor: Option<String>,
) -> Result<ImportBatch, PlaidError> {
    let mut transactions = Vec::new();
    let mut removed = Vec::new();
    let mut seen_cursors = HashSet::new();
    if let Some(cursor) = &cursor {
        seen_cursors.insert(cursor.clone());
    }

    loop {
        let page = provider.fetch_transactions(access_token, cursor).await?;
        // an empty final cursor is valid while Plaid is still preparing the first bank pull
        if page.has_more && page.next_cursor.trim().is_empty() {
            return Err(PlaidError::RequestFailed);
        }
        for payload in page.added.into_iter().chain(page.modified) {
            transactions.push(
                BankTransaction::from_payload(payload).map_err(|_| PlaidError::RequestFailed)?,
            );
        }
        for payload in page.removed {
            let record: RemovedTransaction =
                serde_json::from_value(payload).map_err(|_| PlaidError::RequestFailed)?;
            if record.transaction_id.trim().is_empty() {
                return Err(PlaidError::RequestFailed);
            }
            removed.push(record.transaction_id);
        }
        if !page.has_more {
            return Ok(ImportBatch {
                transactions,
                removed,
                next_cursor: page.next_cursor,
            });
        }

        // a repeated continuation cursor would otherwise keep this request running indefinitely
        if !seen_cursors.insert(page.next_cursor.clone()) {
            return Err(PlaidError::RequestFailed);
        }
        cursor = Some(page.next_cursor);
    }
}

#[derive(Deserialize)]
struct RemovedTransaction {
    transaction_id: String,
}

#[derive(Deserialize)]
struct PlaidTransaction {
    transaction_id: String,
    account_id: String,
    amount: serde_json::Number,
    date: NaiveDate,
    name: String,
    pending: bool,
    pending_transaction_id: Option<String>,
    iso_currency_code: Option<String>,
    unofficial_currency_code: Option<String>,
    personal_finance_category: Option<PlaidCategory>,
}

#[derive(Deserialize)]
struct PlaidCategory {
    primary: String,
    detailed: String,
}

struct BankTransaction {
    provider_transaction_id: String,
    pending_transaction_id: Option<String>,
    provider_account_id: String,
    amount: Decimal,
    date: NaiveDate,
    status: &'static str,
    currency: Option<String>,
    kind: &'static str,
    category: Option<String>,
    description: String,
    original_payload: Value,
}

impl BankTransaction {
    fn from_payload(original_payload: Value) -> Result<Self, ImportError> {
        let record: PlaidTransaction =
            serde_json::from_value(original_payload.clone()).map_err(|_| provider_error())?;
        // parse the decimal representation rather than performing money arithmetic on a binary float
        let amount: Decimal = record
            .amount
            .to_string()
            .parse()
            .map_err(|_| provider_error())?;
        if amount.is_zero()
            || record.transaction_id.trim().is_empty()
            || record.account_id.trim().is_empty()
            || record
                .pending_transaction_id
                .as_ref()
                .is_some_and(|id| id.trim().is_empty())
        {
            return Err(provider_error());
        }
        let primary = record
            .personal_finance_category
            .as_ref()
            .map(|category| category.primary.as_str());
        let kind = match primary {
            Some("INCOME") => "income",
            Some("TRANSFER_IN" | "TRANSFER_OUT") => "transfer",
            None if amount < Decimal::ZERO => "income",
            _ => "expense",
        };

        Ok(Self {
            provider_transaction_id: record.transaction_id,
            pending_transaction_id: record.pending_transaction_id,
            provider_account_id: record.account_id,
            amount: -amount,
            date: record.date,
            status: if record.pending { "pending" } else { "posted" },
            currency: record.iso_currency_code.or(record.unofficial_currency_code),
            kind,
            category: record
                .personal_finance_category
                .map(|category| category.detailed),
            description: record.name,
            original_payload,
        })
    }
}

async fn save_transactions(
    pool: &PgPool,
    expected_connection: &BankConnection,
    mut batch: ImportBatch,
) -> Result<usize, ImportError> {
    let mut database_transaction = pool.begin().await.map_err(database_error)?;

    // serialize writes for this connection, then reject results fetched from an outdated cursor or credential
    let current = sqlx::query_as::<_, BankConnection>(
        "SELECT connection_id, user_id, provider, provider_id, encrypted_access_token, progress_cursor, status
         FROM bank_connections WHERE connection_id = $1 AND user_id = $2 FOR UPDATE",
    )
    .bind(expected_connection.connection_id)
    .bind(expected_connection.user_id)
    .fetch_optional(&mut *database_transaction)
    .await
    .map_err(database_error)?;
    if current.as_ref() != Some(expected_connection) {
        return Err((
            StatusCode::CONFLICT,
            "Bank connection changed; retry the import".to_string(),
        ));
    }

    // a pending record must exist before its explicitly linked posted version, even across page boundaries
    batch
        .transactions
        .sort_by_key(|record| record.status != "pending");
    let mut imported = 0;
    for record in batch.transactions {
        if save_transaction(&mut database_transaction, expected_connection, record)
            .await
            .map_err(database_error)?
        {
            imported += 1;
        }
    }

    // apply removals after pending-to-posted matching; a removed pending ID no longer identifies the posted row
    for provider_transaction_id in batch.removed {
        sqlx::query(
            "UPDATE transactions t SET status = 'removed', updated_at = CURRENT_TIMESTAMP
             FROM imported_transaction_details d
             WHERE d.transaction_id = t.id AND d.connection_id = $1 AND t.user_id = $2
               AND d.provider = $3 AND d.provider_transaction_id = $4",
        )
        .bind(expected_connection.connection_id)
        .bind(expected_connection.user_id)
        .bind(&expected_connection.provider)
        .bind(provider_transaction_id)
        .execute(&mut *database_transaction)
        .await
        .map_err(database_error)?;
    }

    // transactions, removals, and progress become visible together, including a successful empty import
    sqlx::query(
        "UPDATE bank_connections SET progress_cursor = $3, last_synced_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
         WHERE connection_id = $1 AND user_id = $2",
    )
    .bind(expected_connection.connection_id)
    .bind(expected_connection.user_id)
    .bind(batch.next_cursor)
    .execute(&mut *database_transaction)
    .await
    .map_err(database_error)?;
    database_transaction
        .commit()
        .await
        .map_err(database_error)?;
    Ok(imported)
}

#[derive(sqlx::FromRow)]
struct ExistingTransaction {
    id: Uuid,
    kind: String,
    category: Option<String>,
    description: Option<String>,
    kind_overridden: bool,
    category_overridden: bool,
    description_overridden: bool,
}

async fn save_transaction(
    database_transaction: &mut Transaction<'_, Postgres>,
    connection: &BankConnection,
    record: BankTransaction,
) -> Result<bool, sqlx::Error> {
    // only provider identity or explicit pending lineage can match a row; never infer a match from amount or date
    // locking the transaction also makes these writes coordinate with the user's existing edit handler
    let existing = sqlx::query_as::<_, ExistingTransaction>(
        "SELECT t.id, t.kind, t.category, t.description, t.kind_overridden, t.category_overridden, t.description_overridden
         FROM transactions t JOIN imported_transaction_details d ON d.transaction_id = t.id
         WHERE d.connection_id = $1 AND t.user_id = $2 AND d.provider = $3
           AND (d.provider_transaction_id = $4 OR (d.provider_transaction_id = $5 AND d.source_status = 'pending'))
         ORDER BY (d.provider_transaction_id = $4) DESC LIMIT 1 FOR UPDATE OF t, d",
    )
    .bind(connection.connection_id)
    .bind(connection.user_id)
    .bind(&connection.provider)
    .bind(&record.provider_transaction_id)
    .bind(&record.pending_transaction_id)
    .fetch_optional(&mut **database_transaction)
    .await?;

    let Some(existing) = existing else {
        insert_transaction(database_transaction, connection, &record).await?;
        return Ok(true);
    };

    // honor each override independently, including a user-cleared category or description
    let kind = if existing.kind_overridden {
        existing.kind.as_str()
    } else {
        record.kind
    };
    let category = if existing.category_overridden {
        existing.category.as_deref()
    } else {
        record.category.as_deref()
    };
    let description = if existing.description_overridden {
        existing.description.as_deref()
    } else {
        Some(record.description.as_str())
    };
    let semantic_change = kind != existing.kind
        || category != existing.category.as_deref()
        || description != existing.description.as_deref();
    sqlx::query(
        "UPDATE transactions SET amount = $3, date = $4, currency = $5, status = $6,
             kind = $7, category = $8, description = $9, updated_at = CURRENT_TIMESTAMP,
             search_revision = search_revision + CASE WHEN $10 THEN 1 ELSE 0 END
         WHERE id = $1 AND user_id = $2",
    )
    .bind(existing.id)
    .bind(connection.user_id)
    .bind(record.amount)
    .bind(record.date)
    .bind(&record.currency)
    .bind(record.status)
    .bind(kind)
    .bind(category)
    .bind(description)
    .bind(semantic_change)
    .execute(&mut **database_transaction)
    .await?;

    // keep the initial payload, source status, and inferred values intact for a future revert operation
    sqlx::query(
        "UPDATE imported_transaction_details SET provider_transaction_id = $3,
             pending_transaction_id = COALESCE($4, pending_transaction_id), provider_account_id = $5
         WHERE transaction_id = $1 AND connection_id = $2",
    )
    .bind(existing.id)
    .bind(connection.connection_id)
    .bind(&record.provider_transaction_id)
    .bind(&record.pending_transaction_id)
    .bind(&record.provider_account_id)
    .execute(&mut **database_transaction)
    .await?;
    if semantic_change {
        sqlx::query("DELETE FROM transaction_embeddings WHERE transaction_id = $1")
            .bind(existing.id)
            .execute(&mut **database_transaction)
            .await?;
    }
    Ok(false)
}

async fn insert_transaction(
    database_transaction: &mut Transaction<'_, Postgres>,
    connection: &BankConnection,
    record: &BankTransaction,
) -> Result<(), sqlx::Error> {
    let transaction_id: Uuid = sqlx::query_scalar(
        "INSERT INTO transactions (user_id, amount, kind, category, description, date, currency, origin, status)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'imported', $8) RETURNING id",
    )
    .bind(connection.user_id)
    .bind(record.amount)
    .bind(record.kind)
    .bind(&record.category)
    .bind(&record.description)
    .bind(record.date)
    .bind(&record.currency)
    .bind(record.status)
    .fetch_one(&mut **database_transaction)
    .await?;
    sqlx::query(
        "INSERT INTO imported_transaction_details (transaction_id, connection_id, provider, provider_transaction_id,
             pending_transaction_id, provider_account_id, source_status, original_payload, inferred_kind, inferred_category, inferred_description)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(transaction_id)
    .bind(connection.connection_id)
    .bind(&connection.provider)
    .bind(&record.provider_transaction_id)
    .bind(&record.pending_transaction_id)
    .bind(&record.provider_account_id)
    .bind(record.status)
    .bind(&record.original_payload)
    .bind(record.kind)
    .bind(&record.category)
    .bind(&record.description)
    .execute(&mut **database_transaction)
    .await?;

    // existing semantic-search backfill generates missing embeddings after financial data is committed
    Ok(())
}

fn provider_error() -> ImportError {
    (
        StatusCode::BAD_GATEWAY,
        "Could not fetch bank transactions".to_string(),
    )
}

fn credential_error() -> ImportError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "Could not read bank credential".to_string(),
    )
}

fn database_error(_: sqlx::Error) -> ImportError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "Could not save bank transactions".to_string(),
    )
}
