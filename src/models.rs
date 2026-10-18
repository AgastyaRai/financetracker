use sqlx::types::Decimal;
use std::sync::Arc;

use crate::bank_credentials::BankCredentialCipher;
use crate::embeddings::EmbeddingProvider;

/* data structures */

// struct to hold shared application state
#[derive(Clone)]
pub struct AppState {
    // database connection pool
    pub pool: sqlx::PgPool,
    // jwt_secret: String, secret key for signing JWTs
    pub jwt_secret: String,
    // openai api key for generating embeddings
    pub openai_api_key: String,
    // reusable http client for outbound API calls
    pub http_client: reqwest::Client,
    // embedding provider selected by production or test setup
    pub embedding_provider: Arc<dyn EmbeddingProvider>,
    // absent until the backend has Plaid Sandbox credentials and country settings
    pub plaid_provider: Option<Arc<dyn crate::plaid::PlaidProvider>>,
    // validated at startup; absent when credential encryption is not configured
    pub bank_credential_cipher: Option<Arc<BankCredentialCipher>>,
}

// struct for user registration
#[derive(serde::Deserialize)]
pub(crate) struct RegisterUser {
    pub username: String,
    pub email: String,
    pub password: String,
}

// structs for user login
#[derive(serde::Deserialize)]
pub(crate) struct LoginUser {
    pub identifier: String, // can be username or email
    pub password: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct LoginResponse {
    pub user_id: uuid::Uuid,
    pub access_token: String, // for JWT authentication
}

// the frontend supplies only Plaid's temporary token; ownership comes from the authenticated user
#[derive(serde::Deserialize)]
pub(crate) struct ExchangeBankTokenRequest {
    pub public_token: String,
}

// return connection metadata without serializing the provider credential or its encrypted bytes
#[derive(serde::Serialize, sqlx::FromRow)]
pub(crate) struct BankConnectionResponse {
    pub connection_id: uuid::Uuid,
    pub provider: String,
    pub status: String,
}


// enum for transaction kind
#[derive(serde::Deserialize, serde::Serialize, Clone)]
pub(crate) enum TransactionKind {
    Income,
    Expense,
    Transfer,
}

// struct for adding a transaction (request body - no user_id)
#[derive(serde::Deserialize)]
pub(crate) struct AddTransactionRequest {
    pub amount: Decimal,
    pub kind: TransactionKind,
    pub category: Option<String>,
    pub date: chrono::NaiveDate,
    pub description: Option<String>,
}

// struct for editing a transaction using the version shown when the draft was opened
#[derive(serde::Deserialize)]
pub(crate) struct UpdateTransactionRequest {
    // keep the transaction fields at the top level of the request body
    #[serde(flatten)]
    pub transaction: AddTransactionRequest,
    // the save is rejected if the stored transaction has changed since this version
    pub expected_version: i64,
}

// struct for transaction response
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct Transaction {
    pub id: uuid::Uuid,
    pub row_version: i64,
    pub user_id: uuid::Uuid,
    pub amount: Decimal,
    pub kind: TransactionKind,
    pub category: Option<String>,
    pub date: chrono::NaiveDate,
    pub description: Option<String>,
    #[serde(default)]
    pub can_revert: bool,
}

// Counts describe non-removed transactions; missing or outdated embeddings remain outstanding.
#[derive(serde::Serialize, sqlx::FromRow)]
pub(crate) struct IndexingStatus {
    pub total: i64,
    pub indexed: i64,
    pub remaining: i64,
}

// struct for adding/updating a budget (request body - no user_id)
#[derive(serde::Deserialize)]
pub(crate) struct UpsertBudgetRequest {
    pub month: chrono::NaiveDate, // first day of month (e.g., 2026-01-01)
    pub category: String,
    pub amount: Decimal,
}

// struct for budget response
#[derive(serde::Serialize)]
pub(crate) struct Budget {
    pub user_id: uuid::Uuid,
    pub month: chrono::NaiveDate,
    pub category: String,
    pub amount: Decimal,
}

// query params for budgets (optional month filter)
#[derive(serde::Deserialize)]
pub(crate) struct BudgetQuery {
    pub month: Option<chrono::NaiveDate>,
}

// struct for returning budget progress (budget vs spent)
#[derive(serde::Serialize)]
pub(crate) struct BudgetProgress {
    pub category: String,
    pub budget_amount: Decimal,
    pub spent: Decimal,
    pub remaining: Decimal,
}

// struct for JWT claims
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Claims {
    pub sub: String, // we store the user ID as a string in the JWT claims
    pub exp: usize, // expiration time as a unix timestamp
}

// struct for an authenticated user (for extracting user ID from JWT in protected routes)
pub(crate) struct AuthenticatedUser {
    pub user_id: uuid::Uuid,
}

// struct for transaction embedding
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct EmbeddingRequest<'a> {
    pub input: &'a str, // we pass by reference, using lifetime 'a, keeping the referenced string alive long enouhh
    pub model: &'a str,
    pub encoding_format: &'a str,
}

// struct for transaction embedding response from OpenAI API
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct EmbeddingResponse {
    pub data: Vec<EmbeddingData>,
    pub model: String,
    pub object: String, // should be "list"
    pub usage: EmbeddingUsage,
}

// each entry in the data array contains an embedding vector and its metadata
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct EmbeddingData {
    pub embedding: Vec<f32>, // the embedding vector as an array of floats (according to our encoding_format in the request
    pub index: i32,
    pub object: String, // should be "embedding"
}

// the usage field in the response contains token use information
#[derive(serde::Serialize, serde::Deserialize)]
pub (crate) struct EmbeddingUsage {
    pub prompt_tokens: i32,
    pub total_tokens: i32,
}

// struct for semantic search query parameters
#[derive(serde::Deserialize)]
pub(crate) struct SemanticSearchRequest {
    pub query: String, // the search query text
    pub limit: Option<i32>, // optional parameter to specify how many results to return, default to 10 if not provided
    pub summary: Option<bool>, // optional parameter to specify whether to return an AI-generated summary of the search results, default to false if not provided
}

// struct for semantic search result
#[derive(serde::Serialize)]
pub(crate) struct SemanticSearchResult {
    pub transactions: Vec<SemanticTransaction>, // the matching transactions returned by the search with their similarity scores
    pub summary: Option<String>, // an optional AI-generated summary of the search results, if requested
}

// struct for semantic search result transactions paired with their similarity scores
#[derive(serde::Serialize)]
pub(crate) struct SemanticTransaction {
    pub transaction: Transaction,
    pub similarity_score: f64, // the cosine similarity score between the transaction embedding and the search query embedding
}

/* constants */

pub(crate) const JWT_EXPIRATION_HOURS: i64 = 24; // JWT expiration time in hours
