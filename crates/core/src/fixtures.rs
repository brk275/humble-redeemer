//! A local W3C wire-protocol fixture. Never contacts a provider or opens a browser.
use crate::{browser::Browser, model::Provider};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    routing::any,
};
use serde_json::{Value, json};
use std::sync::Arc;

type Handler = Arc<dyn Fn(&str, Value) -> Value + Send + Sync>;
pub struct Fixture {
    task: tokio::task::JoinHandle<()>,
    pub browser: Option<Browser>,
}
impl Fixture {
    pub async fn new(
        provider: Provider,
        handler: impl Fn(&str, Value) -> Value + Send + Sync + 'static,
    ) -> Self {
        async fn command(
            State(handler): State<Handler>,
            Path(path): Path<String>,
            body: Bytes,
        ) -> Json<Value> {
            let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
            Json(json!({"value":handler(&path, body)}))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/session/fixture/{*path}", any(command))
            .with_state(Arc::new(handler) as Handler);
        let task = tokio::spawn(async { axum::serve(listener, router).await.unwrap() });
        Self {
            task,
            browser: Some(Browser::fixture(endpoint, provider)),
        }
    }
    pub async fn service(provider: Provider, response: Value) -> Self {
        Self::new(provider, move |path, _| match path {
            "url" => json!(provider.origin()),
            "cookie" => json!([{ "name":"sessionid", "value":"synthetic-session" }, { "name":"steamLoginSecure", "value":"synthetic-login" }]),
            "execute/async" => response.clone(),
            _ => Value::Null,
        }).await
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
