use crate::utils::internal_error;
use axum::extract::{Path, State};
use axum::Json;
use axum::{body::Body, http::HeaderMap};
use axum::{http::StatusCode, response::IntoResponse, response::Response};
use base64::engine::general_purpose;
use base64::Engine;
use metrics::counter;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sqlx::error::ErrorKind;
use sqlx::Error;
use sqlx::PgPool;
use url::Url;

const DEFAULT_CACHE_CONTROL_HEADER_VALUE: &str =
    "public, max-age=300, s-maxage=300,stale-while-revalidate=300,stale-if-error=300";

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub id: String,
    pub target_url: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkTarget {
    pub target_url: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CountedLinkStatistic {
    pub amount: Option<i64>,
    pub referer: Option<String>,
    pub user_agent: Option<String>,
}

fn generate_id() -> String {
    let randnm = rand::thread_rng().gen_range(0..u32::MAX);
    general_purpose::URL_SAFE_NO_PAD.encode(randnm.to_be_bytes())
}

pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, "Service is healthy")
}

pub async fn redirect(
    State(pool): State<PgPool>,
    Path(requested_link): Path<String>,
    headers: HeaderMap,
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

    let referer_header = headers
        .get("referer")
        .map(|value| value.to_str().unwrap_or_default().to_string());
    let user_agent_header = headers
        .get("user-agent")
        .map(|value| value.to_str().unwrap_or_default().to_string());

    let insert_timeout = tokio::time::Duration::from_millis(300);

    let saved_statistics = tokio::time::timeout(
        insert_timeout,
        sqlx::query(
            r#"INSERT INTO link_statistics (link_id, referer, user_agent) VALUES ($1, $2, $3) RETURNING id"#,
        )
            .bind(&requested_link)
            .bind(&referer_header)
            .bind(&user_agent_header)
            .execute(&pool)
    ).await;

    match saved_statistics {
        Err(elapsed) => tracing::error!("Saving new link resulted in a timeout: {}", elapsed),
        Ok(Err(err)) => {
            tracing::error!("Saving a new link failed with the following error: {}", err)
        }
        _ => tracing::debug!(
            "Persisted new link for link with id {}, referer {}, and user_agent {}",
            requested_link,
            referer_header.unwrap_or_default(),
            user_agent_header.unwrap_or_default()
        ),
    }

    Ok(Response::builder()
        .status(StatusCode::TEMPORARY_REDIRECT)
        .header("Location", link.target_url)
        .header("Cache-Control", DEFAULT_CACHE_CONTROL_HEADER_VALUE)
        .body(Body::empty())
        .expect("This response should always be contructable"))
}

pub async fn create_link(
    State(pool): State<PgPool>,
    Json(new_link): Json<LinkTarget>,
) -> Result<Json<Link>, (StatusCode, String)> {
    let url = Url::parse(&new_link.target_url)
        .map_err(|err| {
            tracing::error!("Invalid URL: {}", err);
            (StatusCode::CONFLICT, "Invalid URL".to_string())
        })?
        .to_string();

    let insert_timeout = tokio::time::Duration::from_millis(300);
    if (1..=3).next().is_some() {
        let new_link_id = generate_id();
        let new_link = tokio::time::timeout(
            insert_timeout,
            sqlx::query_as!(
                Link,
                r#"with inserted_link as (
                INSERT INTO links (id, target_url)
                VALUES ($1, $2)
                RETURNING id, target_url
            )
            SELECT id, target_url FROM inserted_link
            "#,
                &new_link_id,
                &url
            )
            .fetch_one(&pool),
        )
        .await
        .map_err(internal_error)?;

        match new_link {
            Ok(link) => {
                tracing::info!("Created link id {} to {}", &link.id, &link.target_url);
                return Ok(Json(link));
            }
            Err(err) => match err {
                Error::Database(db_err) if db_err.kind() == ErrorKind::UniqueViolation => {
                    return Err((StatusCode::CONFLICT, "Link already exists".to_string()))
                }
                _ => return Err(internal_error(err)),
            },
        }
    }
    tracing::debug!(
        "Could not persist new short link. Exhausted all retries of generating a unique id"
    );
    let uniquelink = counter!("saving_link_failed_no_unique_id");
    uniquelink.increment(1);
    Err((
        StatusCode::INTERNAL_SERVER_ERROR,
        "Could not persist new short link".into(),
    ))
}

pub async fn update_link(
    State(pool): State<PgPool>,
    Path(link_id): Path<String>,
    Json(update_link): Json<LinkTarget>,
) -> Result<Json<Link>, (StatusCode, String)> {
    let url = Url::parse(&update_link.target_url)
        .map_err(|err| {
            tracing::error!("Invalid URL: {}", err);
            (StatusCode::CONFLICT, "Invalid URL".to_string())
        })?
        .to_string();

    let update_timeout = tokio::time::Duration::from_millis(300);

    let link = tokio::time::timeout(
        update_timeout,
        sqlx::query_as!(
            Link,
            r#"with updated_link as
            (UPDATE links SET target_url = $1 WHERE id = $2 RETURNING id, target_url)
            select id,target_url from updated_link"#,
            &url,
            &link_id
        )
        .fetch_one(&pool),
    )
    .await
    .map_err(internal_error)?
    .map_err(internal_error)?;

    tracing::debug!("Updated link id {} to {}", &link.id, &link.target_url);

    Ok(Json(link))
}

pub async fn get_link_statistics(
    State(pool): State<PgPool>,
    Path(link_id): Path<String>,
) -> Result<Json<Vec<CountedLinkStatistic>>, (StatusCode, String)> {
    let fetch_statistics_timeout = tokio::time::Duration::from_millis(300);
    let fetched_statistics = tokio::time::timeout(
        fetch_statistics_timeout,
        sqlx::query_as!(
            CountedLinkStatistic,
            r#"SELECT count(*) as amount, referer, user_agent FROM link_statistics group by link_id,referer,user_agent having link_id = $1"#,
            &link_id
        )
        .fetch_all(&pool),
    ).await.map_err(internal_error)?.map_err(internal_error)?;

    tracing::debug!("Statistics for link with id {} requested", link_id);

    Ok(Json(fetched_statistics))
}
