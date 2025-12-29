use axum::{Router, routing::any};
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let app = Router::new().fallback(any(|| async { "ok" }));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("Listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
