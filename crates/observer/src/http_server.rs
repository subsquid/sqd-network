use std::sync::Arc;

use axum::{
    http::{header, HeaderMap},
    response::IntoResponse,
    routing::get,
};
use prometheus_client::{encoding::text::encode, registry::Registry};

async fn get_metrics(registry: Arc<Registry>) -> impl IntoResponse {
    lazy_static::lazy_static! {
        static ref HEADERS: HeaderMap = {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                "application/openmetrics-text; version=1.0.0; charset=utf-8"
                    .parse()
                    .unwrap(),
            );
            headers
        };
    }

    let mut buffer = String::new();
    encode(&mut buffer, &registry).unwrap();

    (HEADERS.clone(), buffer)
}

pub struct Server {
    router: axum::Router,
}

impl Server {
    pub fn new(metrics_registry: Registry) -> Self {
        let metrics_registry = Arc::new(metrics_registry);
        let router =
            axum::Router::new().route("/metrics", get(move || get_metrics(metrics_registry)));
        Self { router }
    }

    pub async fn run(self, port: u16) -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
        axum::serve(listener, self.router).await?;
        Ok(())
    }
}
