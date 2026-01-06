use std::net::SocketAddr;
use std::time::Duration;

use train_server::cache::{CacheConfig, CachedDarwinClient};

/// Read a secret from environment, preferring `{name}_FILE` over `{name}`.
///
/// If `{name}_FILE` is set, reads the file and returns its contents (trimmed).
/// Panics if the file cannot be read.
/// Otherwise, returns the value of `{name}` if set.
fn read_secret(name: &str) -> Option<String> {
    let file_var = format!("{}_FILE", name);
    if let Ok(path) = std::env::var(&file_var) {
        let contents = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("Failed to read {} from {}: {}", name, path, e));
        return Some(contents.trim().to_string());
    }
    std::env::var(name).ok()
}
use train_server::darwin::{DarwinClient, DarwinClientImpl, DarwinConfig, MockDarwinClient};
use train_server::planner::SearchConfig;
use train_server::stations::{
    StationCache, StationCacheConfig, StationClient, StationClientConfig, StationNames,
};
use train_server::walkable::london_connections;
use train_server::web::{AppState, create_router};

/// How often to refresh station names (24 hours).
const STATION_REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[tokio::main]
async fn main() {
    // Check if we should use mock data
    let use_mock = std::env::var("USE_MOCK_DARWIN")
        .ok()
        .and_then(|v| v.parse::<bool>().ok())
        .unwrap_or(false);

    // Create Darwin client (real or mock)
    let darwin_client = if use_mock {
        println!("Using MOCK Darwin client (loading from data/mock_boards/)");
        let mock =
            MockDarwinClient::new("data/mock_boards").expect("Failed to load mock Darwin data");
        println!(
            "Available mock stations: {:?}",
            mock.available_stations()
                .await
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
        );
        DarwinClientImpl::Mock(mock)
    } else {
        println!("Using REAL Darwin client");
        let api_key = read_secret("DARWIN_API_KEY").unwrap_or_else(|| {
            eprintln!(
                "Error: DARWIN_API_KEY not set. Set USE_MOCK_DARWIN=true to use mock data instead."
            );
            std::process::exit(1);
        });

        let mut darwin_config = DarwinConfig::new(&api_key);

        // Check for optional arrivals API key (separate product on Rail Data Marketplace)
        if let Some(arrivals_key) = read_secret("DARWIN_ARRIVALS_API_KEY") {
            println!("Arrivals API configured");
            darwin_config = darwin_config.with_arrivals_api_key(arrivals_key);
        } else {
            println!(
                "Note: DARWIN_ARRIVALS_API_KEY not set. Train identification at terminus stations won't work.\n\
                 Subscribe to the arrivals product on Rail Data Marketplace for this feature."
            );
        }

        let client = DarwinClient::new(darwin_config).expect("Failed to create Darwin client");
        DarwinClientImpl::Real(client)
    };

    // Create cached client
    let cache_config = CacheConfig::default();
    let cached_darwin = CachedDarwinClient::new(darwin_client, &cache_config);

    // Create walkable connections (using London termini defaults)
    let walkable = london_connections();

    // Create search config
    let search_config = SearchConfig::default();

    // Fetch station names (requires separate Rail Data Marketplace subscription)
    // Uses disk cache to avoid hitting the expensive API on every restart
    let station_names = if use_mock {
        println!("Using mock mode: skipping station names API fetch");
        let station_config = StationClientConfig::new("");
        let station_client =
            StationClient::new(station_config).expect("Failed to create Station client");
        StationNames::empty(station_client)
    } else if let Some(api_key) = read_secret("STATION_API_KEY") {
        let station_config = StationClientConfig::new(&api_key);
        let station_client =
            StationClient::new(station_config).expect("Failed to create Station client");

        // Configure disk cache (default: stations_cache.json, 24h TTL)
        let cache_path = std::env::var("STATION_CACHE_PATH")
            .unwrap_or_else(|_| "stations_cache.json".to_string());
        let cache_config = StationCacheConfig::new(&cache_path);
        let cache = StationCache::new(cache_config);

        println!("Loading station names (cache: {})...", cache_path);
        let (names, from_cache) = StationNames::fetch_with_cache(station_client, cache)
            .await
            .expect("Failed to fetch station names");

        let count = names.len().await;
        if from_cache {
            println!("Loaded {} station names from cache", count);
        } else {
            println!(
                "Fetched {} station names from API (cached for next restart)",
                count
            );
        }
        names
    } else {
        println!("STATION_API_KEY not set, using empty station names");
        let station_config = StationClientConfig::new("");
        let station_client =
            StationClient::new(station_config).expect("Failed to create Station client");
        StationNames::empty(station_client)
    };

    // Spawn background task to refresh station names daily
    let station_names_refresh = station_names.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(STATION_REFRESH_INTERVAL);
        interval.tick().await; // First tick is immediate, skip it
        loop {
            interval.tick().await;
            match station_names_refresh.refresh().await {
                Ok(count) => println!("Refreshed station names: {} stations", count),
                Err(e) => eprintln!("Failed to refresh station names: {}", e),
            }
        }
    });

    // Build app state
    let state = AppState::new(cached_darwin, walkable, search_config, station_names);

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
