use crate::models::{AppState, Transaction};
use axum::http::StatusCode;
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestUserMessageArgs,
    CreateChatCompletionRequestArgs,
};

// helper function for generating an AI summary of a list of transactions using the OpenAI API
pub async fn generate_semantic_search_summary(
    state: &AppState,
    query: &str,
    transactions: &[Transaction],
) -> Result<String, (StatusCode, String)> {
    let mut transaction_records = String::new();

    let mut total_price = rust_decimal::Decimal::ZERO;
    
    for (i, transaction)in transactions.iter().enumerate() {


        let kind = match transaction.kind {
            crate::models::TransactionKind::Expense => "Expense",
            crate::models::TransactionKind::Income => "Income",
        };

        let record = format!(
           "Transaction {}: Date: {}, Kind: {}, Category: {}, Description: {}, Amount: {}\n",
            i + 1,
            transaction.date,
            kind,
            transaction.category.clone().unwrap_or_else(|| "None".to_string()),
            transaction.description.clone().unwrap_or_else(|| "None".to_string()),
            transaction.amount
        );

        total_price += transaction.amount;

        transaction_records.push_str(&record);
    }

    let user_prompt = format!(
        "A user searched their finance transactions with the query: '{}'. \n\n\
         Here are the matching transactions returned by the search: \n{}\n\
         Here is the net total amount for these transactions: {}.\n\n\
         Write a concise 2-3 sentence summary of what these results show. Do not \
         invent any transactions or numbers that are not present.",
        query,
        transaction_records,
        total_price
    );

    let config = OpenAIConfig::new().with_api_key(state.openai_api_key.clone());
    let client = Client::with_config(config);

    let system_message = ChatCompletionRequestSystemMessageArgs::default()
            .content("You are a helpful financial assistant that summarizes retrieved transactions clearly and concisely.")
            .build()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .into();
    
    let user_message = ChatCompletionRequestUserMessageArgs::default()
            .content(user_prompt)
            .build()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .into();

    let chat_request = CreateChatCompletionRequestArgs::default()
        .model("gpt-4o-mini")
        .messages([system_message, user_message])
        .temperature(0.2)
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response = client.chat().create(chat_request)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let summary = response.choices.first()
        .and_then(|choice| choice.message.content.as_ref())
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "No content in OpenAI response".to_string()))?;
    
    Ok(summary.clone())
}