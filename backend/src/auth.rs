use crate::error::{ApiResult, AppError};
use crate::state::AppState;
use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts},
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
}

pub fn make_token(secret: &str) -> anyhow::Result<String> {
    let exp = chrono::Utc::now().timestamp() as usize + 7 * 24 * 3600;
    let claims = Claims {
        sub: "admin".into(),
        exp,
    };
    Ok(encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?)
}

pub fn verify_token(secret: &str, token: &str) -> Result<Claims, AppError> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map(|d| d.claims)
    .map_err(|_| AppError::unauthorized("登录已过期，请重新登录"))
}

pub struct AuthUser {
    #[allow(dead_code)]
    pub username: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        let header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AppError::unauthorized("未登录"))?;
        let token = header
            .strip_prefix("Bearer ")
            .ok_or_else(|| AppError::unauthorized("无效令牌"))?;
        let claims = verify_token(&state.jwt_secret, token)?;
        Ok(AuthUser {
            username: claims.sub,
        })
    }
}
