use crate::utils::internal_error;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::{http::StatusCode, response::IntoResponse, response::Response};
use sqlx::PgPool;
const DEFAULT_CACHE_CONTROL_HEADER_VALUE: &str =
    "public, max-age=300, s-maxage=300,stale-while-revalidate=300,stale-if-error=300";
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub id: String,
    pub target_url: String,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkTarget {
    pub target_url: String,
}
pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, "Service is healthy")
}

pub async fn redirect(
    State(pool): State<PgPool>,
    Path(requested_link): Path<String>,
) -> Result<Response, (StatusCode, String)> {
    let select_timeout = tokio::time::Duration::from_millis(300);
    let link = tokio::time::timeout(
        select_timeout,
        sqlx::query_as!(Link, r#"SELECT * FROM links WHERE id = $1"#, requested_link)
            .fetch_optional(&pool),
    )
    .await
    .map_err(internal_error)?
    .map_err(internal_error)?
    .ok_or_else(|| "Not found".to_string())
    .map_err(|err| (StatusCode::NOT_FOUND, err))?;

    tracing::debug!(
        "Redirecting link id {} to {}",
        requested_link,
        link.target_url
    );

    Ok(Response::builder()
        .status(StatusCode::TEMPORARY_REDIRECT)
        .header("Location", link.target_url)
        .header("Cache-Control", DEFAULT_CACHE_CONTROL_HEADER_VALUE)
        .body(Body::empty())
        .expect("This response should always be contructable"))
}

pub async fn create_link(
    State(pool): State<pool>,
    Json(new_link): Json<LinkTarget>,
) -> Result<Json<Link>, (StatusCode, String)> {
    let url = Url::parse(&new_link.target_url).map_err(|err| {
        tracing::error!("Invalid URL: {}", err);
        (StatusCode::BAD_REQUEST, "Invalid URL".to_string())
    })?;
}
