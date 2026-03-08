use crate::models::{AddTransactionRequest, AppState, EmbeddingRequest, TransactionKind};
use axum::http::StatusCode;
use pgvector::Vector;

impl AddTransactionRequest {

    // helper function to turn Transaction into parsable string for embedding
    pub fn transaction_string_embedding(&self) -> String {

        /* 
            For now, we're just using the transaction type, category and description
            for the embedding vector, as these are the most semantically relevant fields for understanding the transaction.

            Could potentially come back to amount in the future, but would require more thought on how to be represented
            in a way that's meaningful + some testing to see if it actually helps.

            As an idea for the future, we could categorize the amount into very rough buckets, and just indicate 
            'small', 'medium', 'large' or something like that in the embedding string, to give the model a sense of scale.
        */

        // get the transaction type, category, and description (with defaults if not provided) and format them into a string for embedding
        let transaction_type = match self.kind {
            TransactionKind::Expense => "Expense",
            TransactionKind::Income => "Income",
        };
        
        let category = self.category.as_deref().unwrap_or("Uncategorized");
        let description = self.description.as_deref().unwrap_or("No description");

        let embedding_string = format!(
            "kind: {}\n category: {}\n description: {}",
            transaction_type, category, description
        );

        embedding_string
    }

}

// function to generate embeddings from text using OpenAI API
pub async fn generate_transaction_embedding(
    state: &AppState,
    embedding_text: &str,
) -> Result<Vec<f32>, (StatusCode, String)> {
    // openai expects headers Auth Bearer <key> and Content-Type application/json
    // body fields input, model, and encoding_format
    let embedding_request = EmbeddingRequest {
        input: embedding_text,
        model: "text-embedding-3-small",
        encoding_format: "float"
    };

    let response = state.http_client
        .post("https://api.openai.com/v1/embeddings")
        .bearer_auth(&state.openai_api_key)
        .json(&embedding_request)
        .send()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("OpenAI API error: Status {}, Response {}", status, text)));
    }
        
    let embedding_response = response
        .json::<crate::models::EmbeddingResponse>()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;    

    let embedding = embedding_response
        .data
        .into_iter()
        .next()
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "No embedding data returned".to_string()))?
        .embedding;

    Ok(embedding)
}

// helper function to store a transaction embedding into the table in the database
pub async fn store_transaction_embedding(
    state: &AppState,
    transaction_id: uuid::Uuid,
    user_id: uuid::Uuid,
    embedding_text: &str,
    embedding: Vec<f32>,
) -> Result<(), (StatusCode, String)> {
    // we use the pgvector extension to store the embedding vector in the database
    sqlx::query(
        "INSERT INTO transaction_embeddings (transaction_id, user_id, embedding_text, embedding) VALUES ($1, $2, $3, $4)"
    )
    .bind(transaction_id)
    .bind(user_id)
    .bind(embedding_text)
    .bind(Vector::from(embedding)) // insert as a pgvector type
    .execute(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(())
}

// unit test
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TransactionKind;
    use chrono::NaiveDate;
    use rust_decimal::Decimal;

    // check that the embedding string includes the transaction type, category and description correctly
    #[test]
    fn test_transaction_string_embedding() {
        let req = AddTransactionRequest {
            amount: Decimal::new(1234, 2), // $12.34
            date: NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            category: Some("Food".to_string()),
            description: Some("Lunch at cafe".to_string()),
            kind: TransactionKind::Expense,
        };

        let embedding_string = req.transaction_string_embedding();

        assert_eq!(
            embedding_string,
            "kind: Expense\n category: Food\n description: Lunch at cafe"
        )
    }

    // test that the embedding string handles missing optional fields correctly
    #[test]
    fn test_transaction_string_embedding_missing_fields() {
        let req = AddTransactionRequest {
            amount: Decimal::new(5000, 2), // $50.00
            date: NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            category: None,
            description: None,
            kind: TransactionKind::Income,
        };

        let embedding_string = req.transaction_string_embedding();

        assert_eq!(
            embedding_string,
            "kind: Income\n category: Uncategorized\n description: No description"
        )
    }
}
