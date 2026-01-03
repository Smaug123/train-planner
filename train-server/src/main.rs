use std::net::SocketAddr;

use train_server::cache::{CacheConfig, CachedDarwinClient};
use train_server::darwin::{DarwinClient, DarwinConfig};
use train_server::planner::SearchConfig;
use train_server::walkable::london_connections;
use train_server::web::{AppState, create_router};

#[tokio::main]
async fn main() {
    // Get credentials from environment
    let username = std::env::var("DARWIN_USERNAME").unwrap_or_else(|_| {
        eprintln!("Warning: DARWIN_USERNAME not set. API calls will fail.");
        String::new()
    });
    let password = std::env::var("DARWIN_PASSWORD").unwrap_or_else(|_| {
        eprintln!("Warning: DARWIN_PASSWORD not set. API calls will fail.");
        String::new()
    });

    // Create Darwin client
    let darwin_config = DarwinConfig::new(username, password);
    let darwin_client = DarwinClient::new(darwin_config).expect("Failed to create Darwin client");

    // Create cached client
    let cache_config = CacheConfig::default();
    let cached_darwin = CachedDarwinClient::new(darwin_client, &cache_config);

    // Create walkable connections (using London termini defaults)
    let walkable = london_connections();

    // Create search config
    let search_config = SearchConfig::default();

    // Build app state
    let state = AppState::new(cached_darwin, walkable, search_config);

    // Create router
    let app = create_router(state);

    // Bind and serve
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("Train Journey Planner listening on http://{addr}");
    println!();
    println!("Open http://{addr} in your browser for the web interface.");
    println!();
    println!("API Endpoints:");
    println!("  GET  /health          - Health check");
    println!("  GET  /about           - About page");
    println!("  GET  /search/service  - Search for services");
    println!("  POST /journey/plan    - Plan a journey");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
