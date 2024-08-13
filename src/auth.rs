use crate::utils::internal_error;
use axum::http::StatusCode;
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::IntoResponse,
};
use metrics::counter;
use sha3::{Digest, Sha3_256};
use sqlx::PgPool;

struct Setting {
    #[allow(dead_code)]
    id: String,
    encrypted_global_api_key: String,
}

pub async fn auth(
    State(pool): State<PgPool>,
    req: Request,
    next: Next,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let labels = [("uri", format!("{}", req.uri()))];
    let request_counter = counter!("unauthenticated_req_count", &labels);

    let api_key = req
        .headers()
        .get("X-API-KEY")
        .map(|value| value.to_str().unwrap_or_default())
        .ok_or_else(|| {
            tracing::error!("Unauthorized request: missing x-api-key header");
            request_counter.increment(1);
            (StatusCode::UNAUTHORIZED, "Unauthorized".into())
        })?;

    let fetch_settings_timeout = tokio::time::Duration::from_millis(300);

    let setting = tokio::time::timeout(
        fetch_settings_timeout,
        sqlx::query_as!(
            Setting,
            r#"SELECT * FROM settings WHERE encrypted_global_api_key = $1"#,
            api_key
        )
        .fetch_one(&pool),
    )
    .await
    .map_err(internal_error)?
    .map_err(internal_error)?;

    let mut hasher = Sha3_256::new();
    hasher.update(api_key.as_bytes());
    let provided_api_key = hasher.finalize();

    if setting.encrypted_global_api_key != format!("{provided_api_key:x}") {
        tracing::error!("Unauthorized request: invalid x-api-key header");
        request_counter.increment(1);
        return Err((StatusCode::UNAUTHORIZED, "Unauthorized".into()));
    }

    Ok(next.run(req).await)
}
