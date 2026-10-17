mod common;

use async_trait::async_trait;
use axum::http::StatusCode;
use financetracker::embedding_worker::{
    EmbeddingBatchResult, EmbeddingWorkerConfig, process_embedding_batch, run_embedding_worker,
    stop_embedding_worker,
};
use financetracker::embeddings::{EmbeddingProvider, store_transaction_embedding};
use financetracker::{AppState, build_app};
use pgvector::Vector;
use serde_json::Value;
use sqlx::PgPool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;

enum FirstResponse {
    Success,
    Failure,
    Wait,
}

// Control the first provider response and record every input; later requests always succeed.
struct TestEmbeddingProvider {
    first_response: FirstResponse,
    requests: Mutex<Vec<String>>,
    started: Notify,
    release: Notify,
}

impl TestEmbeddingProvider {
    fn new(first_response: FirstResponse) -> Self {
        Self {
            first_response,
            requests: Mutex::new(Vec::new()),
            started: Notify::new(),
            release: Notify::new(),
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl EmbeddingProvider for TestEmbeddingProvider {
    async fn generate_embedding(
        &self,
        _http_client: &reqwest::Client,
        _openai_api_key: &str,
        embedding_text: &str,
    ) -> Result<Vec<f32>, (StatusCode, String)> {
        // Release the recording lock before pausing so the test can inspect the in-flight request.
        let call = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(embedding_text.to_string());
            requests.len()
        };
        if call == 1 {
            match self.first_response {
                FirstResponse::Success => {}
                FirstResponse::Failure => {
                    return Err((
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Simulated embedding outage".to_string(),
                    ));
                }
                FirstResponse::Wait => {
                    self.started.notify_one();
                    self.release.notified().await;
                }
            }
        }
        Ok(test_embedding())
    }
}

fn test_embedding() -> Vec<f32> {
    let mut embedding = vec![0.0; 1536];
    embedding[0] = 1.0;
    embedding
}

async fn test_state(pool: PgPool, provider: Arc<TestEmbeddingProvider>) -> AppState {
    let mut state = common::setup_app_state_with_embedding_provider(provider).await;
    state.pool = pool;
    state
}

async fn register_user(state: &AppState) -> Uuid {
    let app = build_app(state.clone());
    let (username, password) = common::create_and_register_test_user(&app).await;
    common::login_test_user(&app, &username, &password).await.0
}

// Seed saved financial rows directly so setup does not generate embeddings through the create handler.
async fn save_transaction(state: &AppState, user_id: Uuid, number: u128, status: &str) -> Uuid {
    let id = Uuid::from_u128(number);
    sqlx::query(
        "INSERT INTO transactions (id, user_id, amount, kind, category, date, description, origin, status, currency)
         VALUES ($1, $2, -12.50, 'expense', 'Food', '2026-10-17', $3, 'imported', $4, 'CAD')",
    )
    .bind(id)
    .bind(user_id)
    .bind(format!("Batch transaction {number}"))
    .bind(status)
    .execute(&state.pool)
    .await
    .unwrap();
    id
}

async fn saved_transactions(state: &AppState) -> Vec<Value> {
    sqlx::query_scalar("SELECT to_jsonb(t) FROM transactions t ORDER BY id")
        .fetch_all(&state.pool)
        .await
        .unwrap()
}

#[derive(Debug, PartialEq, sqlx::FromRow)]
struct StoredEmbedding {
    user_id: Uuid,
    embedding_text: String,
    embedding: Vector,
    search_revision: i32,
    embedding_model: String,
}

async fn saved_embedding(state: &AppState, transaction_id: Uuid) -> Option<StoredEmbedding> {
    sqlx::query_as(
        "SELECT user_id, embedding_text, embedding, search_revision, embedding_model
         FROM transaction_embeddings WHERE transaction_id = $1",
    )
    .bind(transaction_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap()
}

// Each test owns a separate database: a worker's scan must not pick up another test's unfinished rows.
#[sqlx::test(migrations = "backend/migrations")]
async fn test_batch_respects_limit_and_skips_current_and_removed_transactions(pool: PgPool) {
    let provider = Arc::new(TestEmbeddingProvider::new(FirstResponse::Success));
    let state = test_state(pool, provider.clone()).await;
    let owner = register_user(&state).await;
    let other_owner = register_user(&state).await;

    // Put skipped rows first, then three eligible rows across two owners, including one stale embedding.
    let current = save_transaction(&state, owner, 1, "posted").await;
    let removed = save_transaction(&state, owner, 2, "removed").await;
    let missing = save_transaction(&state, owner, 3, "posted").await;
    let pending = save_transaction(&state, other_owner, 4, "pending").await;
    let stale = save_transaction(&state, owner, 5, "posted").await;
    sqlx::query("UPDATE transactions SET kind = 'income', category = NULL, description = NULL WHERE id = $1")
        .bind(pending).execute(&state.pool).await.unwrap();
    sqlx::query("UPDATE transactions SET kind = 'transfer', category = NULL WHERE id = $1")
        .bind(stale)
        .execute(&state.pool)
        .await
        .unwrap();
    store_transaction_embedding(
        &state,
        current,
        owner,
        "kind: Expense\n category: Food\n description: Batch transaction 1",
        test_embedding(),
        1,
        "text-embedding-3-small",
    )
    .await
    .unwrap();
    store_transaction_embedding(
        &state,
        stale,
        owner,
        "Older transaction text",
        test_embedding(),
        0,
        "legacy:unknown",
    )
    .await
    .unwrap();
    let current_embedding = saved_embedding(&state, current).await;
    let stale_embedding = saved_embedding(&state, stale).await;
    let transactions = saved_transactions(&state).await;

    // A limit of two means two provider requests, not two scanned rows or all remaining work.
    assert_eq!(
        process_embedding_batch(&state, 2).await.unwrap(),
        EmbeddingBatchResult {
            indexed: 2,
            failed: 0,
            discarded: 0
        }
    );
    assert_eq!(
        provider.requests(),
        vec![
            "kind: Expense\n category: Food\n description: Batch transaction 3",
            "kind: Income\n category: Uncategorized\n description: No description",
        ]
    );
    assert_eq!(
        saved_embedding(&state, missing).await.unwrap().user_id,
        owner
    );
    assert_eq!(
        saved_embedding(&state, pending).await.unwrap().user_id,
        other_owner
    );
    assert_eq!(saved_embedding(&state, stale).await, stale_embedding);
    assert_eq!(saved_embedding(&state, current).await, current_embedding);
    assert!(saved_embedding(&state, removed).await.is_none());

    // The next pass discovers the stale row without queue state and stores its latest revision and model.
    assert_eq!(
        process_embedding_batch(&state, 2).await.unwrap(),
        EmbeddingBatchResult {
            indexed: 1,
            failed: 0,
            discarded: 0
        }
    );
    let rebuilt = saved_embedding(&state, stale).await.unwrap();
    assert_eq!(rebuilt.search_revision, 1);
    assert_eq!(rebuilt.embedding_model, "text-embedding-3-small");
    assert_eq!(
        rebuilt.embedding_text,
        "kind: Transfer\n category: Uncategorized\n description: Batch transaction 5"
    );
    assert_eq!(rebuilt.embedding.to_vec(), test_embedding());
    assert_eq!(provider.requests().len(), 3);
    assert_eq!(saved_transactions(&state).await, transactions);
    assert_eq!(saved_embedding(&state, current).await, current_embedding);
    assert!(saved_embedding(&state, removed).await.is_none());
}

#[sqlx::test(migrations = "backend/migrations")]
async fn test_provider_failure_preserves_financial_data_and_leaves_work_for_retry(pool: PgPool) {
    let provider = Arc::new(TestEmbeddingProvider::new(FirstResponse::Failure));
    let state = test_state(pool, provider.clone()).await;
    let owner = register_user(&state).await;
    let failed = save_transaction(&state, owner, 1, "posted").await;
    let successful = save_transaction(&state, owner, 2, "posted").await;
    let transactions = saved_transactions(&state).await;

    // A failed request cannot abort the rest of this batch or change the already saved financial rows.
    assert_eq!(
        process_embedding_batch(&state, 2).await.unwrap(),
        EmbeddingBatchResult {
            indexed: 1,
            failed: 1,
            discarded: 0
        }
    );
    assert_eq!(provider.requests().len(), 2);
    assert!(saved_embedding(&state, failed).await.is_none());
    let successful_embedding = saved_embedding(&state, successful).await;
    assert!(successful_embedding.is_some());
    assert_eq!(saved_transactions(&state).await, transactions);

    // A later pass retries only the missing result; it must not regenerate the successful one.
    assert_eq!(
        process_embedding_batch(&state, 2).await.unwrap(),
        EmbeddingBatchResult {
            indexed: 1,
            failed: 0,
            discarded: 0
        }
    );
    assert_eq!(provider.requests().len(), 3);
    assert_eq!(
        saved_embedding(&state, failed)
            .await
            .unwrap()
            .search_revision,
        1
    );
    assert_eq!(
        saved_embedding(&state, successful).await,
        successful_embedding
    );
    assert_eq!(saved_transactions(&state).await, transactions);
}

#[sqlx::test(migrations = "backend/migrations")]
async fn test_concurrent_edit_rejects_old_embedding_and_later_batch_indexes_new_revision(
    pool: PgPool,
) {
    let provider = Arc::new(TestEmbeddingProvider::new(FirstResponse::Wait));
    let state = test_state(pool, provider.clone()).await;
    let owner = register_user(&state).await;
    let transaction_id = save_transaction(&state, owner, 1, "posted").await;
    let batch_state = state.clone();
    let mut batch = tokio::spawn(async move { process_embedding_batch(&batch_state, 1).await });

    // Wait until the old text is being embedded; an implementation that does no work must fail immediately.
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! {
            _ = provider.started.notified() => {}
            result = &mut batch => panic!("batch finished before requesting an embedding: {result:?}"),
        }
    }).await.expect("batch must reach the provider");

    // Commit a source correction while the provider is paused. This also detects a lock held across the external call.
    let edit = tokio::time::timeout(Duration::from_secs(5),
        sqlx::query("UPDATE transactions SET description = 'Corrected groceries', search_revision = search_revision + 1,
                        updated_at = CURRENT_TIMESTAMP WHERE id = $1")
            .bind(transaction_id).execute(&state.pool),
    ).await;
    provider.release.notify_one();
    edit.expect("embedding generation must not block an edit")
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), batch)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        result,
        EmbeddingBatchResult {
            indexed: 0,
            failed: 0,
            discarded: 1
        }
    );
    assert!(saved_embedding(&state, transaction_id).await.is_none());

    // The latest revision remains discoverable and its vector uses the corrected text without any financial write.
    let transactions = saved_transactions(&state).await;
    assert_eq!(
        process_embedding_batch(&state, 1).await.unwrap(),
        EmbeddingBatchResult {
            indexed: 1,
            failed: 0,
            discarded: 0
        }
    );
    let current = saved_embedding(&state, transaction_id).await.unwrap();
    assert_eq!(current.search_revision, 2);
    assert_eq!(
        current.embedding_text,
        "kind: Expense\n category: Food\n description: Corrected groceries"
    );
    assert_eq!(
        provider.requests(),
        vec![
            "kind: Expense\n category: Food\n description: Batch transaction 1",
            "kind: Expense\n category: Food\n description: Corrected groceries",
        ]
    );
    assert_eq!(saved_transactions(&state).await, transactions);
}

#[sqlx::test(migrations = "backend/migrations")]
async fn test_completed_batch_does_not_call_provider_again(pool: PgPool) {
    let provider = Arc::new(TestEmbeddingProvider::new(FirstResponse::Success));
    let state = test_state(pool, provider.clone()).await;
    let owner = register_user(&state).await;
    let transaction_id = save_transaction(&state, owner, 1, "posted").await;

    // Finish one pass normally before checking that subsequent scans leave both the row and vector alone.
    assert_eq!(
        process_embedding_batch(&state, 10).await.unwrap(),
        EmbeddingBatchResult {
            indexed: 1,
            failed: 0,
            discarded: 0
        }
    );
    let embedding = saved_embedding(&state, transaction_id).await;
    let transactions = saved_transactions(&state).await;
    let requests = provider.requests();
    assert_eq!(requests.len(), 1);

    // Current embeddings are the completion record; no separate queue entry is needed to avoid repeat calls.
    assert_eq!(
        process_embedding_batch(&state, 10).await.unwrap(),
        EmbeddingBatchResult::default()
    );
    assert_eq!(provider.requests(), requests);
    assert_eq!(saved_embedding(&state, transaction_id).await, embedding);
    assert_eq!(saved_transactions(&state).await, transactions);
}

// Record request timing and cancellation so retries and shutdown can be checked without real API calls.
#[derive(Clone, Copy)]
enum LifecycleResponse {
    Success,
    Failure,
    Wait,
}

#[derive(Clone)]
struct RecordedRequest {
    text: String,
    at: tokio::time::Instant,
}

struct LifecycleProvider {
    response: Mutex<LifecycleResponse>,
    failing_text: Option<String>,
    requests: Mutex<Vec<RecordedRequest>>,
    active: AtomicUsize,
    max_active: AtomicUsize,
    cancelled: AtomicUsize,
    release: Notify,
}

impl LifecycleProvider {
    fn new(response: LifecycleResponse) -> Self {
        Self {
            response: Mutex::new(response),
            failing_text: None,
            requests: Mutex::new(Vec::new()),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
            release: Notify::new(),
        }
    }

    fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn respond_with(&self, response: LifecycleResponse) {
        *self.response.lock().unwrap() = response;
    }
}

// Dropping a pending provider future must release it, rather than leave detached work running.
struct ActiveRequest<'a> {
    provider: &'a LifecycleProvider,
    completed: bool,
}

impl Drop for ActiveRequest<'_> {
    fn drop(&mut self) {
        self.provider.active.fetch_sub(1, Ordering::SeqCst);
        if !self.completed {
            self.provider.cancelled.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[async_trait]
impl EmbeddingProvider for LifecycleProvider {
    async fn generate_embedding(
        &self,
        _http_client: &reqwest::Client,
        _openai_api_key: &str,
        embedding_text: &str,
    ) -> Result<Vec<f32>, (StatusCode, String)> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        let mut request = ActiveRequest {
            provider: self,
            completed: false,
        };
        self.requests.lock().unwrap().push(RecordedRequest {
            text: embedding_text.to_string(),
            at: tokio::time::Instant::now(),
        });
        let response = if self
            .failing_text
            .as_deref()
            .is_some_and(|text| embedding_text.ends_with(text))
        {
            LifecycleResponse::Failure
        } else {
            *self.response.lock().unwrap()
        };

        // Yield on successful calls so overlapping worker calls would be visible in max_active.
        match response {
            LifecycleResponse::Success => tokio::task::yield_now().await,
            LifecycleResponse::Failure => {
                request.completed = true;
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Simulated embedding outage".to_string(),
                ));
            }
            LifecycleResponse::Wait => self.release.notified().await,
        }
        request.completed = true;
        Ok(test_embedding())
    }
}

// Pause Tokio's clock, but inhibit automatic clock jumps while PostgreSQL performs real network I/O.
// The blocking task sleeps on a channel; the test alone advances time, and dropping this guard frees it.
struct ManualClock {
    release: Option<std::sync::mpsc::Sender<()>>,
}

impl ManualClock {
    async fn pause() -> Self {
        tokio::time::pause();
        let (release, receiver) = std::sync::mpsc::channel();
        let (ready, started) = tokio::sync::oneshot::channel();
        tokio::task::spawn_blocking(move || {
            let _ = ready.send(());
            let _ = receiver.recv();
        });
        started.await.unwrap();
        Self {
            release: Some(release),
        }
    }
}

impl Drop for ManualClock {
    fn drop(&mut self) {
        self.release.take();
        tokio::time::resume();
    }
}

async fn lifecycle_state(pool: PgPool, provider: Arc<LifecycleProvider>) -> AppState {
    let mut state = common::setup_app_state_with_embedding_provider(provider).await;
    state.pool = pool;
    state
}

// Observe completed worker database operations without adding a production progress-reporting API.
fn observed_worker_state(state: &AppState) -> (AppState, Arc<AtomicUsize>) {
    let released = Arc::new(AtomicUsize::new(0));
    let counter = released.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_release(move |_, _| {
            let counter = counter.clone();
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(true)
            })
        })
        .connect_lazy_with(state.pool.connect_options().as_ref().clone());
    let mut worker_state = state.clone();
    worker_state.pool = pool;
    (worker_state, released)
}

fn start_worker(
    state: AppState,
    config: EmbeddingWorkerConfig,
) -> (watch::Sender<bool>, JoinHandle<()>) {
    let (stop, shutdown) = watch::channel(false);
    (
        stop,
        tokio::spawn(run_embedding_worker(state, config, shutdown)),
    )
}

// Use a wall-clock deadline for harness failures: Tokio time stays paused until the test advances it.
async fn wait_for_worker(task: &JoinHandle<()>, ready: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(
            !task.is_finished(),
            "Worker stopped before doing the expected work"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "Worker did not reach the expected operation"
        );
        tokio::task::yield_now().await;
    }
}

async fn wait_for_indexed(state: &AppState, task: &JoinHandle<()>, expected: i64) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM transaction_embeddings")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        if count == expected {
            break;
        }
        assert!(
            !task.is_finished(),
            "Worker stopped before saving the expected embeddings"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "Expected {expected} embeddings; found {count}"
        );
        tokio::task::yield_now().await;
    }
}

// Tokio rounds deadlines up to its next millisecond tick. Cross that tick explicitly while auto-advance is disabled.
async fn reach_deadline(after: Duration) {
    tokio::time::advance(after + Duration::from_millis(1)).await;
}

fn assert_retry_delay(actual: Duration, expected: Duration) {
    assert!(actual >= expected, "Retry ran before its deadline");
    assert!(actual <= expected + Duration::from_millis(1), "Retry missed the next timer tick");
}

async fn settle_clock() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

async fn wait_for_stop(task: &JoinHandle<()>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !task.is_finished() {
        assert!(
            std::time::Instant::now() < deadline,
            "Worker did not stop without another clock advance"
        );
        tokio::task::yield_now().await;
    }
}

#[sqlx::test(migrations = "backend/migrations")]
async fn test_worker_starts_immediately_and_drains_sequential_batches(pool: PgPool) {
    let provider = Arc::new(LifecycleProvider::new(LifecycleResponse::Success));
    let state = lifecycle_state(pool, provider.clone()).await;
    let owner = register_user(&state).await;
    for number in 1..=5 {
        save_transaction(&state, owner, number, "posted").await;
    }
    let before = saved_transactions(&state).await;
    let _clock = ManualClock::pause().await;

    // Starting the task is the only trigger; five rows require three batches with a limit of two.
    let config = EmbeddingWorkerConfig {
        batch_limit: 2,
        ..Default::default()
    };
    let (stop, task) = start_worker(state.clone(), config);
    wait_for_indexed(&state, &task, 5).await;
    let requests = provider.requests();
    assert_eq!(requests.len(), 5);
    for (number, request) in (1..=5).zip(requests) {
        assert!(
            request
                .text
                .ends_with(&format!("Batch transaction {number}"))
        );
    }
    assert_eq!(provider.max_active.load(Ordering::SeqCst), 1);
    assert_eq!(saved_transactions(&state).await, before);
    stop.send(true).unwrap();
    stop_embedding_worker(task, config.shutdown_grace)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "backend/migrations")]
async fn test_worker_polls_idle_database_and_discovers_new_transactions(pool: PgPool) {
    let provider = Arc::new(LifecycleProvider::new(LifecycleResponse::Success));
    let state = lifecycle_state(pool, provider.clone()).await;
    let owner = register_user(&state).await;
    let (worker_state, released) = observed_worker_state(&state);
    let _clock = ManualClock::pause().await;
    let config = EmbeddingWorkerConfig::default();
    let (stop, task) = start_worker(worker_state, config);
    wait_for_worker(&task, || released.load(Ordering::SeqCst) == 1).await;

    // Saving a transaction does not notify the worker; it must find the row on its next five-second scan.
    let id = save_transaction(&state, owner, 1, "posted").await;
    tokio::time::advance(Duration::from_secs(4)).await;
    settle_clock().await;
    assert!(provider.requests().is_empty());
    assert_eq!(
        released.load(Ordering::SeqCst),
        1,
        "Idle worker must not busy-poll"
    );
    reach_deadline(Duration::from_secs(1)).await;
    wait_for_indexed(&state, &task, 1).await;
    assert!(saved_embedding(&state, id).await.is_some());
    stop.send(true).unwrap();
    stop_embedding_worker(task, config.shutdown_grace)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "backend/migrations")]
async fn test_worker_retry_backoff_doubles_caps_resets_and_handles_database_errors(pool: PgPool) {
    let provider = Arc::new(LifecycleProvider::new(LifecycleResponse::Failure));
    let state = lifecycle_state(pool, provider.clone()).await;
    let owner = register_user(&state).await;
    save_transaction(&state, owner, 1, "posted").await;
    let (worker_state, released) = observed_worker_state(&state);
    let _clock = ManualClock::pause().await;
    let config = EmbeddingWorkerConfig::default();
    let (stop, task) = start_worker(worker_state, config);
    wait_for_worker(&task, || provider.requests().len() == 1).await;

    // An outage retries at 5, 10, 20, 40, then at most 60 seconds; there is no request before each deadline.
    for (attempt, seconds) in [5, 10, 20, 40, 60, 60].into_iter().enumerate() {
        tokio::time::advance(Duration::from_secs(seconds - 1)).await;
        settle_clock().await;
        assert_eq!(provider.requests().len(), attempt + 1);
        reach_deadline(Duration::from_secs(1)).await;
        wait_for_worker(&task, || provider.requests().len() == attempt + 2).await;
        let requests = provider.requests();
        assert_retry_delay(
            requests[attempt + 1].at - requests[attempt].at,
            Duration::from_secs(seconds),
        );
    }

    // A successful batch resets backoff, so an unrelated later failure starts again with five seconds.
    provider.respond_with(LifecycleResponse::Success);
    reach_deadline(Duration::from_secs(60)).await;
    wait_for_indexed(&state, &task, 1).await;
    wait_for_worker(&task, || released.load(Ordering::SeqCst) >= 11).await;
    save_transaction(&state, owner, 2, "posted").await;
    provider.respond_with(LifecycleResponse::Failure);
    reach_deadline(Duration::from_secs(5)).await;
    wait_for_worker(&task, || provider.requests().len() == 9).await;
    tokio::time::advance(Duration::from_secs(4)).await;
    settle_clock().await;
    assert_eq!(provider.requests().len(), 9);
    reach_deadline(Duration::from_secs(1)).await;
    wait_for_worker(&task, || provider.requests().len() == 10).await;
    let requests = provider.requests();
    assert_retry_delay(requests[9].at - requests[8].at, Duration::from_secs(5));
    stop.send(true).unwrap();
    stop_embedding_worker(task, config.shutdown_grace)
        .await
        .unwrap();

    // Temporarily hide this disposable test's table to make selection fail, then restore it for recovery.
    let before = saved_transactions(&state).await;
    sqlx::query("ALTER TABLE transactions RENAME TO unavailable_transactions")
        .execute(&state.pool)
        .await
        .unwrap();
    let (worker_state, released) = observed_worker_state(&state);
    let (stop, task) = start_worker(worker_state, config);
    wait_for_worker(&task, || released.load(Ordering::SeqCst) == 1).await;
    tokio::time::advance(Duration::from_secs(4)).await;
    settle_clock().await;
    assert_eq!(released.load(Ordering::SeqCst), 1);
    reach_deadline(Duration::from_secs(1)).await;
    wait_for_worker(&task, || released.load(Ordering::SeqCst) == 2).await;
    sqlx::query("ALTER TABLE unavailable_transactions RENAME TO transactions")
        .execute(&state.pool)
        .await
        .unwrap();
    provider.respond_with(LifecycleResponse::Success);
    tokio::time::advance(Duration::from_secs(9)).await;
    settle_clock().await;
    assert_eq!(released.load(Ordering::SeqCst), 2);
    reach_deadline(Duration::from_secs(1)).await;
    wait_for_indexed(&state, &task, 2).await;
    assert_eq!(saved_transactions(&state).await, before);
    stop.send(true).unwrap();
    stop_embedding_worker(task, config.shutdown_grace)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "backend/migrations")]
async fn test_worker_moves_past_permanently_failing_rows(pool: PgPool) {
    let mut fake = LifecycleProvider::new(LifecycleResponse::Success);
    fake.failing_text = Some("Batch transaction 1".to_string());
    let provider = Arc::new(fake);
    let state = lifecycle_state(pool, provider.clone()).await;
    let owner = register_user(&state).await;
    for number in 1..=3 {
        save_transaction(&state, owner, number, "posted").await;
    }
    let before = saved_transactions(&state).await;
    let _clock = ManualClock::pause().await;
    let config = EmbeddingWorkerConfig {
        batch_limit: 1,
        ..Default::default()
    };
    let (stop, task) = start_worker(state.clone(), config);
    wait_for_worker(&task, || provider.requests().len() == 1).await;

    // The failed first row remains eligible, but the next batch must reach rows two and three.
    reach_deadline(Duration::from_secs(5)).await;
    wait_for_indexed(&state, &task, 2).await;
    let requests = provider.requests();
    for (number, request) in (1..=3).zip(requests.iter()) {
        assert!(
            request
                .text
                .ends_with(&format!("Batch transaction {number}"))
        );
    }
    assert!(saved_embedding(&state, Uuid::from_u128(1)).await.is_none());
    assert!(saved_embedding(&state, Uuid::from_u128(2)).await.is_some());
    assert!(saved_embedding(&state, Uuid::from_u128(3)).await.is_some());
    assert_eq!(saved_transactions(&state).await, before);
    stop.send(true).unwrap();
    stop_embedding_worker(task, config.shutdown_grace)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "backend/migrations")]
async fn test_worker_times_out_hung_provider_and_continues_the_batch(pool: PgPool) {
    let provider = Arc::new(LifecycleProvider::new(LifecycleResponse::Wait));
    let state = lifecycle_state(pool, provider.clone()).await;
    let owner = register_user(&state).await;
    let first = save_transaction(&state, owner, 1, "posted").await;
    let second = save_transaction(&state, owner, 2, "posted").await;
    let before = saved_transactions(&state).await;
    let _clock = ManualClock::pause().await;
    let config = EmbeddingWorkerConfig::default();
    let (stop, task) = start_worker(state.clone(), config);
    wait_for_worker(&task, || provider.requests().len() == 1).await;

    // Only the first request hangs; its ten-second timeout must free the next item in the same batch.
    provider.respond_with(LifecycleResponse::Success);
    tokio::time::advance(Duration::from_secs(9)).await;
    settle_clock().await;
    assert_eq!(provider.requests().len(), 1);
    assert_eq!(provider.cancelled.load(Ordering::SeqCst), 0);
    reach_deadline(Duration::from_secs(1)).await;
    wait_for_indexed(&state, &task, 1).await;
    assert_eq!(provider.cancelled.load(Ordering::SeqCst), 1);
    assert!(saved_embedding(&state, first).await.is_none());
    assert!(saved_embedding(&state, second).await.is_some());
    assert_eq!(saved_transactions(&state).await, before);
    stop.send(true).unwrap();
    stop_embedding_worker(task, config.shutdown_grace)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "backend/migrations")]
async fn test_worker_shutdown_interrupts_idle_and_retry_waits(pool: PgPool) {
    let provider = Arc::new(LifecycleProvider::new(LifecycleResponse::Failure));
    let state = lifecycle_state(pool, provider.clone()).await;
    let owner = register_user(&state).await;
    let _clock = ManualClock::pause().await;
    let config = EmbeddingWorkerConfig::default();

    // Stop once during an empty scan's idle wait and once during an outage's retry wait.
    for idle in [true, false] {
        if !idle {
            save_transaction(&state, owner, 1, "posted").await;
        }
        let (worker_state, released) = observed_worker_state(&state);
        let (stop, task) = start_worker(worker_state, config);
        wait_for_worker(&task, || released.load(Ordering::SeqCst) == 1).await;
        assert_eq!(provider.requests().len(), usize::from(!idle));
        let stopped_at = tokio::time::Instant::now();
        stop.send(true).unwrap();
        wait_for_stop(&task).await;
        stop_embedding_worker(task, config.shutdown_grace)
            .await
            .unwrap();
        assert_eq!(tokio::time::Instant::now(), stopped_at);
    }
}

#[sqlx::test(migrations = "backend/migrations")]
async fn test_worker_shutdown_finishes_current_item_without_starting_another(pool: PgPool) {
    let provider = Arc::new(LifecycleProvider::new(LifecycleResponse::Wait));
    let state = lifecycle_state(pool, provider.clone()).await;
    let owner = register_user(&state).await;
    let first = save_transaction(&state, owner, 1, "posted").await;
    let second = save_transaction(&state, owner, 2, "posted").await;
    let _clock = ManualClock::pause().await;
    let config = EmbeddingWorkerConfig::default();
    let (stop, task) = start_worker(state.clone(), config);
    wait_for_worker(&task, || provider.requests().len() == 1).await;

    // Shutdown gives the active item time to finish and store its result, but blocks the next item.
    stop.send(true).unwrap();
    let stopping = tokio::spawn(stop_embedding_worker(task, config.shutdown_grace));
    settle_clock().await;
    tokio::time::advance(Duration::from_secs(9)).await;
    settle_clock().await;
    assert!(!stopping.is_finished());
    provider.release.notify_one();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !stopping.is_finished() {
        assert!(
            std::time::Instant::now() < deadline,
            "Shutdown did not finish the released item"
        );
        tokio::task::yield_now().await;
    }
    stopping.await.unwrap().unwrap();
    assert!(saved_embedding(&state, first).await.is_some());
    assert!(saved_embedding(&state, second).await.is_none());
    assert_eq!(provider.requests().len(), 1);
    assert_eq!(provider.cancelled.load(Ordering::SeqCst), 0);
}

#[sqlx::test(migrations = "backend/migrations")]
async fn test_worker_shutdown_cancels_joins_and_restart_recovers_unfinished_work(pool: PgPool) {
    let provider = Arc::new(LifecycleProvider::new(LifecycleResponse::Wait));
    let state = lifecycle_state(pool, provider.clone()).await;
    let owner = register_user(&state).await;
    for number in 1..=2 {
        save_transaction(&state, owner, number, "posted").await;
    }
    let before = saved_transactions(&state).await;
    let _clock = ManualClock::pause().await;
    // A longer request timeout proves that shutdown's own ten-second deadline performs cancellation.
    let config = EmbeddingWorkerConfig {
        request_timeout: Duration::from_secs(60),
        ..Default::default()
    };
    let (stop, task) = start_worker(state.clone(), config);
    wait_for_worker(&task, || provider.requests().len() == 1).await;
    stop.send(true).unwrap();
    let stopping = tokio::spawn(stop_embedding_worker(task, config.shutdown_grace));
    settle_clock().await;
    tokio::time::advance(Duration::from_secs(9)).await;
    settle_clock().await;
    assert!(!stopping.is_finished());
    assert_eq!(provider.active.load(Ordering::SeqCst), 1);
    reach_deadline(Duration::from_secs(1)).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !stopping.is_finished() {
        assert!(
            std::time::Instant::now() < deadline,
            "Shutdown exceeded its grace period"
        );
        tokio::task::yield_now().await;
    }
    stopping.await.unwrap().unwrap();
    assert_eq!(
        provider.active.load(Ordering::SeqCst),
        0,
        "The cancelled worker must be joined"
    );
    assert_eq!(provider.cancelled.load(Ordering::SeqCst), 1);
    assert!(saved_embedding(&state, Uuid::from_u128(1)).await.is_none());
    assert_eq!(saved_transactions(&state).await, before);

    // No claim or queue record needs repair: a fresh task finds both missing embeddings in the database.
    provider.respond_with(LifecycleResponse::Success);
    let (stop, task) = start_worker(state.clone(), config);
    wait_for_indexed(&state, &task, 2).await;
    assert_eq!(provider.requests().len(), 3);
    assert_eq!(saved_transactions(&state).await, before);
    stop.send(true).unwrap();
    stop_embedding_worker(task, config.shutdown_grace)
        .await
        .unwrap();
}
