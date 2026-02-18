use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use crate::models::{AppState, AuthenticatedUser, Claims};

/* helper functions */

// helper function to verify a JWT and returns the user ID
pub fn verify_jwt(token: &str, secret: &str) -> Result<(uuid::Uuid, usize), String> {
    let decoding_key = DecodingKey::from_secret(secret.as_bytes());
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;

    // validate the token and decode the claims
    let token_data = jsonwebtoken::decode::<Claims>(token, &decoding_key, &validation)
        .map_err(|e| e.to_string())?;

    // parse the user ID from the subject claim
    let user_id = uuid::Uuid::parse_str(&token_data.claims.sub)
        .map_err(|e| e.to_string())?;
    let exp = token_data.claims.exp;

    Ok((user_id, exp))
}

/* extractor functions */

// this extractor is used in protected routes to extract the user ID from the JWT in the Authorization header
#[axum::async_trait]
impl axum::extract::FromRequestParts<AppState> for AuthenticatedUser {

    type Rejection = (axum::http::StatusCode, String);

    async fn from_request_parts(parts: &mut axum::http::request::Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        // get the Authorization header as a string
        let auth_header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .ok_or((
                axum::http::StatusCode::UNAUTHORIZED,
                "Missing Authorization header".to_string(),
            ))?;

        // extract token from "Bearer <token>" format by removing the prefix
        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or((
                axum::http::StatusCode::UNAUTHORIZED,
                "Invalid Authorization format, expected: Bearer <token>".to_string(),
            ))?;

        // verify the JWT and extract the user ID
        let (user_id, _exp) = verify_jwt(token, &state.jwt_secret)
            .map_err(|_e| {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    "Invalid or expired token".to_string(),
                )
            })?;

        Ok(AuthenticatedUser { user_id })
    }

}
