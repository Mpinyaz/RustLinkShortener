mod routes;
mod utils;
use crate::routes::*;
use axum::routing::get;
use axum::Router;
use axum_prometheus::PrometheusMetricLayer;
use dotenvy::dotenv;
use sqlx::postgres::PgPoolOptions;
use std::error::Error;
use tower_http::trace::TraceLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenv().ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "link_shortner=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let db = PgPoolOptions::new()
        .max_connections(20)
        .connect(&db_url)
        .await?;
    let (prometheus_layer, metric_handle) = PrometheusMetricLayer::pair();
    let app = Router::new()
        .route("/:id", get(redirect))
        .route("/metrics", get(|| async move { metric_handle.render() }))
        .route("/health", get(health))
        .layer(TraceLayer::new_for_http())
        .layer(prometheus_layer)
        .with_state(db);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("Could not initialize listener");
    tracing::debug!(
        "Listening on: {}",
        listener.local_addr().expect("Could not get address")
    );
    axum::serve(listener, app)
        .await
        .expect("could not start server");
    Ok(())
}
