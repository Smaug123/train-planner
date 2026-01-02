use std::net::SocketAddr;

use train_server::cache::{CacheConfig, CachedDarwinClient};
use train_server::darwin::{DarwinClient, DarwinConfig};
use train_server::planner::SearchConfig;
use train_server::walkable::london_connections;
use train_server::web::{create_router, AppState};

#[tokio::main]
async fn main() {
    // Get API key from environment
    let api_key = std::env::var("DARWIN_API_KEY").unwrap_or_else(|_| {
        eprintln!("Warning: DARWIN_API_KEY not set. API calls will fail.");
        String::new()
    });

    // Create Darwin client
    let darwin_config = DarwinConfig::new(api_key);
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
    println!("Endpoints:");
    println!("  GET  /health          - Health check");
    println!("  GET  /search/service  - Search for services");
    println!("  POST /journey/plan    - Plan a journey");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
