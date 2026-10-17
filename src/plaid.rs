use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

// return only the values the frontend needs to initialize Plaid Link
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct LinkToken {
    pub link_token: String,
    pub expiration: DateTime<Utc>,
}

// backend-only exchange result; the access token must never be serialized into an API response
#[derive(serde::Deserialize)]
pub struct ExchangedConnection {
    #[serde(rename = "item_id")]
    pub provider_id: String,
    pub access_token: String,
}

// retain complete provider records until the importer saves their original payloads
#[derive(serde::Deserialize)]
pub struct TransactionPage {
    pub added: Vec<serde_json::Value>,
    pub modified: Vec<serde_json::Value>,
    pub removed: Vec<serde_json::Value>,
    pub next_cursor: String,
    pub has_more: bool,
}

// provider failures carry no upstream body or credentials into the handler
#[derive(Debug, PartialEq, Eq)]
pub enum PlaidError {
    RequestFailed,
    LoginRequired,
}

#[derive(serde::Deserialize)]
struct PlaidErrorResponse {
    error_code: String,
}

#[async_trait]
pub trait PlaidProvider: Send + Sync {
    async fn create_link_token(&self, user_id: Uuid) -> Result<LinkToken, PlaidError>;
    async fn exchange_public_token(&self, public_token: &str) -> Result<ExchangedConnection, PlaidError>;
    async fn fetch_transactions(&self, access_token: &str, cursor: Option<String>) -> Result<TransactionPage, PlaidError>;
}

pub const SANDBOX_BASE_URL: &str = "https://sandbox.plaid.com";

pub struct PlaidClient {
    http_client: reqwest::Client,
    base_url: String,
    client_id: String,
    secret: String,
    country_codes: Vec<String>,
}

impl PlaidClient {
    // production selects Sandbox; HTTP tests supply their isolated localhost server
    pub fn new(
        http_client: reqwest::Client,
        base_url: &str,
        client_id: String,
        secret: String,
        country_codes: Vec<String>,
    ) -> Self {
        Self {
            http_client,
            base_url: base_url.trim_end_matches('/').to_string(),
            client_id,
            secret,
            country_codes,
        }
    }
}

#[async_trait]
impl PlaidProvider for PlaidClient {
    async fn create_link_token(&self, user_id: Uuid) -> Result<LinkToken, PlaidError> {
        // identify the user from our JWT and keep product/country settings on the server
        let response = self.http_client
            .post(format!("{}/link/token/create", self.base_url))
            .header("PLAID-CLIENT-ID", &self.client_id)
            .header("PLAID-SECRET", &self.secret)
            .json(&serde_json::json!({
                "user": { "client_user_id": user_id.to_string() },
                "client_name": "FinanceTracker",
                "products": ["transactions"],
                "country_codes": self.country_codes,
                "language": "en",
            }))
            .send()
            .await
            .map_err(|_| PlaidError::RequestFailed)?
            .error_for_status()
            .map_err(|_| PlaidError::RequestFailed)?;

        // require a usable token and timestamp without exposing upstream error bodies
        let token = response.json::<LinkToken>().await.map_err(|_| PlaidError::RequestFailed)?;
        if token.link_token.trim().is_empty() {
            return Err(PlaidError::RequestFailed);
        }

        Ok(token)
    }


    async fn exchange_public_token(&self, public_token: &str) -> Result<ExchangedConnection, PlaidError> {
        let response = self.http_client
            .post(format!("{}/item/public_token/exchange", self.base_url))
            .header("PLAID-CLIENT-ID", &self.client_id)
            .header("PLAID-SECRET", &self.secret)
            .json(&serde_json::json!({ "public_token": public_token }))
            .send()
            .await
            .map_err(|_| PlaidError::RequestFailed)?
            .error_for_status()
            .map_err(|_| PlaidError::RequestFailed)?;

        // both fields are needed to encrypt and retain the credential under its provider identity
        let connection = response.json::<ExchangedConnection>().await.map_err(|_| PlaidError::RequestFailed)?;
        if connection.access_token.trim().is_empty() || connection.provider_id.trim().is_empty() {
            return Err(PlaidError::RequestFailed);
        }

        Ok(connection)
    }

    async fn fetch_transactions(&self, access_token: &str, cursor: Option<String>) -> Result<TransactionPage, PlaidError> {
        let mut request = serde_json::json!({ "access_token": access_token });
        if let Some(cursor) = cursor {
            request["cursor"] = serde_json::Value::String(cursor);
        }

        let response = self.http_client
            .post(format!("{}/transactions/sync", self.base_url))
            .header("PLAID-CLIENT-ID", &self.client_id)
            .header("PLAID-SECRET", &self.secret)
            .json(&request)
            .send()
            .await
            .map_err(|_| PlaidError::RequestFailed)?;

        // retain only the recognized login error; never pass upstream messages or credentials to the importer
        if response.status() == reqwest::StatusCode::BAD_REQUEST {
            let error = response
                .json::<PlaidErrorResponse>()
                .await
                .map_err(|_| PlaidError::RequestFailed)?;
            return Err(match error.error_code.as_str() {
                "ITEM_LOGIN_REQUIRED" => PlaidError::LoginRequired,
                _ => PlaidError::RequestFailed,
            });
        }
        let response = response
            .error_for_status()
            .map_err(|_| PlaidError::RequestFailed)?;

        // require the complete page contract; callers retain raw records for their saved provenance
        response.json::<TransactionPage>().await.map_err(|_| PlaidError::RequestFailed)
    }
}
