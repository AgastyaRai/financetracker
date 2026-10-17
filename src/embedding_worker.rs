use crate::embeddings::{TransactionEmbedding, store_transaction_embedding_if_current};
use crate::models::AppState;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::{JoinError, JoinHandle};
use uuid::Uuid;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct EmbeddingBatchResult {
    pub indexed: usize,
    pub failed: usize,
    pub discarded: usize,
}

#[derive(sqlx::FromRow)]
struct TransactionToEmbed {
    id: Uuid,
    user_id: Uuid,
    kind: String,
    category: Option<String>,
    description: Option<String>,
    search_revision: i32,
}

// process one bounded batch; starting and repeating batches belongs to the caller
pub async fn process_embedding_batch(
    state: &AppState,
    batch_limit: u32,
) -> Result<EmbeddingBatchResult, sqlx::Error> {
    Ok(
        process_embedding_batch_after(state, batch_limit, None, None, None)
            .await?
            .result,
    )
}

struct EmbeddingBatchPage {
    result: EmbeddingBatchResult,
    last_id: Option<Uuid>,
}

async fn process_embedding_batch_after(
    state: &AppState,
    batch_limit: u32,
    after_id: Option<Uuid>,
    shutdown: Option<&watch::Receiver<bool>>,
    request_timeout: Option<Duration>,
) -> Result<EmbeddingBatchPage, sqlx::Error> {
    // revisions identify unfinished work, including work left behind by a failed or stopped worker
    let transactions = sqlx::query_as::<_, TransactionToEmbed>(
        "SELECT t.id, t.user_id, t.kind, t.category, t.description, t.search_revision
         FROM transactions t
         WHERE t.status <> 'removed'
           AND ($2::uuid IS NULL OR t.id > $2)
           AND NOT EXISTS (
               SELECT 1 FROM transaction_embeddings e
               WHERE e.transaction_id = t.id AND e.user_id = t.user_id
                 AND e.search_revision = t.search_revision
           )
         ORDER BY t.id
         LIMIT $1",
    )
    .bind(i64::from(batch_limit))
    .bind(after_id)
    .fetch_all(&state.pool)
    .await?;

    // finish the database read before calling the provider; edits and imports remain free to commit
    let mut result = EmbeddingBatchResult::default();
    let mut last_id = None;
    for transaction in transactions {
        // Let an active request and its guarded write finish; shutdown prevents starting the next item.
        if shutdown.is_some_and(is_stopping) {
            break;
        }
        last_id = Some(transaction.id);
        let embedding = TransactionEmbedding::generate(
            state,
            &transaction.kind,
            transaction.category.as_deref(),
            transaction.description.as_deref(),
            transaction.search_revision,
        );
        let embedding = if let Some(timeout) = request_timeout {
            match tokio::time::timeout(timeout, embedding).await {
                Ok(embedding) => embedding,
                Err(_) => {
                    result.failed += 1;
                    continue;
                }
            }
        } else {
            embedding.await
        };
        let embedding = match embedding {
            Ok(embedding) => embedding,
            Err(_) => {
                result.failed += 1;
                continue;
            }
        };

        // publish only a still-current result; failed or superseded work remains discoverable for a later pass
        match store_transaction_embedding_if_current(
            state,
            transaction.id,
            transaction.user_id,
            embedding,
        )
        .await
        {
            Ok(true) => result.indexed += 1,
            Ok(false) => result.discarded += 1,
            Err(_) => result.failed += 1,
        }
    }
    Ok(EmbeddingBatchPage { result, last_id })
}

#[derive(Clone, Copy, Debug)]
pub struct EmbeddingWorkerConfig {
    pub batch_limit: u32,
    pub poll_interval: Duration,
    pub retry_initial: Duration,
    pub retry_max: Duration,
    pub request_timeout: Duration,
    pub shutdown_grace: Duration,
}

impl Default for EmbeddingWorkerConfig {
    fn default() -> Self {
        Self {
            batch_limit: 10,
            poll_interval: Duration::from_secs(5),
            retry_initial: Duration::from_secs(5),
            retry_max: Duration::from_secs(60),
            request_timeout: Duration::from_secs(10),
            shutdown_grace: Duration::from_secs(10),
        }
    }
}

fn is_stopping(shutdown: &watch::Receiver<bool>) -> bool {
    *shutdown.borrow() || shutdown.has_changed().is_err()
}

// Both idle polling and outage backoff must wake immediately when shutdown is requested.
async fn wait_before_batch(delay: Duration, shutdown: &mut watch::Receiver<bool>) -> bool {
    if is_stopping(shutdown) {
        return false;
    }
    tokio::select! {
        biased;
        _ = shutdown.changed() => false,
        _ = tokio::time::sleep(delay) => true,
    }
}

pub async fn run_embedding_worker(
    state: AppState,
    config: EmbeddingWorkerConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut after_id = None;
    let mut retry_delay = config.retry_initial;
    while !is_stopping(&shutdown) {
        let page = process_embedding_batch_after(
            &state,
            config.batch_limit,
            after_id,
            Some(&shutdown),
            Some(config.request_timeout),
        )
        .await;

        let delay = match page {
            Err(_) => Some(retry_delay),
            Ok(page) => {
                // Advance past attempted rows even after failure, then wrap to retry them on the next sweep.
                // Otherwise a permanently failing first batch can hide every later transaction.
                if let Some(last_id) = page.last_id {
                    after_id = Some(last_id);
                    if page.result.failed > 0 {
                        Some(retry_delay)
                    } else {
                        retry_delay = config.retry_initial;
                        None
                    }
                } else if after_id.is_some() {
                    after_id = None;
                    None
                } else {
                    // An empty scan does not reset an outage's backoff or add a second retry delay.
                    if !wait_before_batch(config.poll_interval, &mut shutdown).await {
                        return;
                    }
                    None
                }
            }
        };

        if let Some(delay) = delay {
            if !wait_before_batch(delay, &mut shutdown).await {
                return;
            }
            retry_delay = retry_delay.saturating_mul(2).min(config.retry_max);
        }
    }
}

// The caller signals shutdown first. Await cancellation as well as normal completion before closing the pool.
pub async fn stop_embedding_worker(
    mut task: JoinHandle<()>,
    grace: Duration,
) -> Result<(), JoinError> {
    match tokio::time::timeout(grace, &mut task).await {
        Ok(result) => result,
        Err(_) => {
            task.abort();
            match task.await {
                Err(error) if error.is_cancelled() => Ok(()),
                result => result,
            }
        }
    }
}
