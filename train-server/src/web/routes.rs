//! HTTP route handlers.

use std::sync::Arc;

use askama::Template;
use axum::body::Bytes;
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use chrono::{Local, NaiveDate, Timelike};
use tower_http::services::ServeDir;

use crate::domain::{CallIndex, Crs, Service};
use crate::planner::{Planner, SearchError, SearchRequest};

use super::dto::*;
use super::state::AppState;
use super::templates::*;

/// Create the application router.
///
/// `static_dir` is the path to the static assets directory.
pub fn create_router(state: AppState, static_dir: &str) -> Router {
    Router::new()
        .route("/", get(index_page))
        .route("/health", get(health))
        .route("/about", get(about_page))
        .route("/api/stations/search", get(search_stations))
        .route("/search/service", get(search_service))
        .route("/identify", get(identify_train))
        .route("/journey/plan", post(plan_journey))
        .nest_service("/static", ServeDir::new(static_dir))
        .with_state(state)
}

/// Health check endpoint.
async fn health() -> &'static str {
    "ok"
}

/// Index page with search form.
async fn index_page() -> impl IntoResponse {
    Html(
        IndexTemplate
            .render()
            .unwrap_or_else(|e| format!("Template error: {}", e)),
    )
}

/// About page.
async fn about_page() -> impl IntoResponse {
    Html(
        AboutTemplate
            .render()
            .unwrap_or_else(|e| format!("Template error: {}", e)),
    )
}

/// Search stations by name or CRS code.
async fn search_stations(
    State(state): State<AppState>,
    Query(req): Query<StationSearchRequest>,
) -> Json<StationSearchResponse> {
    let limit = req.limit.unwrap_or(10).min(50);
    let matches = state.station_names.search(&req.q, limit).await;

    let stations = matches
        .into_iter()
        .map(|m| StationSearchResult {
            crs: m.crs,
            name: m.name,
        })
        .collect();

    Json(StationSearchResponse { stations })
}

/// Check if request accepts HTML.
fn accepts_html(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}

/// Search for services from a station.
async fn search_service(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(req): Query<SearchServiceRequest>,
) -> Result<Response, AppError> {
    // Parse origin CRS
    let origin_crs = Crs::parse_normalized(&req.origin).map_err(|_| AppError::BadRequest {
        message: format!("Invalid origin CRS: {}", req.origin),
    })?;

    // Parse optional destination CRS
    let dest_crs = req
        .destination
        .as_ref()
        .map(|d| Crs::parse_normalized(d))
        .transpose()
        .map_err(|_| AppError::BadRequest {
            message: format!(
                "Invalid destination CRS: {}",
                req.destination.as_deref().unwrap_or("")
            ),
        })?;

    // Get current time info
    let now = Local::now();
    let date = now.date_naive();
    let current_mins = (now.time().hour() * 60 + now.time().minute()) as u16;

    // Fetch departures
    let services = match dest_crs {
        Some(dest) => state
            .darwin
            .get_departures_to(&origin_crs, date, current_mins, 0, 120, &dest)
            .await
            .map_err(AppError::from)?,
        None => {
            let all = state
                .darwin
                .get_departures_with_details(&origin_crs, date, current_mins, 0, 120)
                .await
                .map_err(AppError::from)?;
            all.iter().cloned().collect()
        }
    };

    // Filter by headcode if specified
    let services: Vec<_> = if let Some(ref headcode) = req.headcode {
        let headcode_upper = headcode.to_uppercase();
        services
            .into_iter()
            .filter(|s| {
                s.service
                    .headcode
                    .as_ref()
                    .is_some_and(|h| h.to_string() == headcode_upper)
            })
            .collect()
    } else {
        services
    };

    // Return HTML or JSON based on Accept header
    if accepts_html(&headers) {
        let service_views: Vec<ServiceView> = services
            .iter()
            .map(|s| ServiceView::from_service(&s.service))
            .collect();

        let template = ServiceListTemplate {
            services: service_views,
        };
        let html = template.render().map_err(|e| AppError::Internal {
            message: format!("Template error: {}", e),
        })?;

        Ok(Html(html).into_response())
    } else {
        // JSON response
        let results: Vec<ServiceResult> = services
            .iter()
            .map(|s| ServiceResult::from_service(&s.service))
            .collect();

        Ok(Json(SearchServiceResponse { services: results }).into_response())
    }
}

/// Identify the user's current train by next station and terminus.
async fn identify_train(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(req): Query<IdentifyTrainWebRequest>,
) -> Result<Response, AppError> {
    use super::rtt::rtt_search_url_default;
    use crate::domain::MatchConfidence;
    use crate::identify::filter_and_rank_matches;

    // Parse next station CRS
    let next_station =
        Crs::parse_normalized(&req.next_station).map_err(|_| AppError::BadRequest {
            message: format!("Invalid next station CRS: {}", req.next_station),
        })?;

    // Parse optional terminus CRS
    let terminus = req
        .terminus
        .as_ref()
        .filter(|t| !t.is_empty())
        .map(|t| Crs::parse_normalized(t))
        .transpose()
        .map_err(|_| AppError::BadRequest {
            message: format!(
                "Invalid terminus CRS: {}",
                req.terminus.as_deref().unwrap_or("")
            ),
        })?;

    // Get current time info
    let now = Local::now();
    let date = now.date_naive();
    let current_mins = (now.time().hour() * 60 + now.time().minute()) as u16;

    // Query both boards and merge results.
    // - Departures board has subsequent calling points (where train is going)
    // - Arrivals board finds set-down-only trains that don't appear on departures
    // For services appearing on both, prefer departures data (has future stops).
    let (departures, arrivals) = tokio::join!(
        state
            .darwin
            .get_departures_with_details(&next_station, date, current_mins, 0, 30),
        state
            .darwin
            .get_arrivals_with_details(&next_station, date, current_mins, 0, 30)
    );

    let departures = departures.unwrap_or_default();
    let arrivals = arrivals.unwrap_or_default();

    // Merge: use departures as base, add arrivals-only services.
    // Departures have subsequent calling points; arrivals catch set-down-only trains.
    let departure_ids: std::collections::HashSet<_> = departures
        .iter()
        .map(|s| s.service.service_ref.darwin_id.as_str())
        .collect();

    // Identify arrivals-only services (set-down-only trains not on departures board)
    let arrivals_only: Vec<_> = arrivals
        .iter()
        .filter(|s| !departure_ids.contains(s.service.service_ref.darwin_id.as_str()))
        .collect();

    // For arrivals-only services, fetch full service details to get subsequent calling points.
    // This is an extra API call per service, but these are rare (set-down-only trains).
    let mut enhanced_arrivals = Vec::new();
    for svc in arrivals_only {
        let service_id = &svc.service.service_ref.darwin_id;
        match state.darwin.get_service_details(service_id).await {
            Ok(details) => {
                match crate::darwin::convert_service_details(
                    &details,
                    service_id,
                    &next_station,
                    date,
                ) {
                    Ok(converted) => enhanced_arrivals.push(std::sync::Arc::new(converted)),
                    Err(e) => {
                        eprintln!(
                            "Warning: failed to convert service details for {}: {}",
                            service_id, e
                        );
                        // Fall back to the original arrivals data
                        enhanced_arrivals.push(svc.clone());
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "Warning: failed to fetch service details for {}: {}",
                    service_id, e
                );
                // Fall back to the original arrivals data
                enhanced_arrivals.push(svc.clone());
            }
        }
    }

    let services: Vec<_> = departures
        .iter()
        .cloned()
        .chain(enhanced_arrivals)
        .collect();

    // Filter and rank matches using the extracted logic
    let matches = filter_and_rank_matches(&services, terminus.as_ref());

    // Return HTML or JSON based on Accept header
    if accepts_html(&headers) {
        let match_views: Vec<TrainMatchView> = matches
            .iter()
            .map(|m| {
                let dep_time = m
                    .service
                    .candidate
                    .expected_departure
                    .unwrap_or(m.service.candidate.scheduled_departure);

                // Get arrival info at the board station (next station)
                let board_call = m
                    .service
                    .service
                    .calls
                    .get(m.service.service.board_station_idx.0);

                let next_station_name = board_call
                    .map(|c| c.station_name.clone())
                    .unwrap_or_else(|| next_station.as_str().to_string());

                // For the board station, prefer arrival times but fall back to departure
                // times. Departures boards often don't include arrival times (sta/eta),
                // only departure times (std/etd), so we need this fallback to correctly
                // show delay status.
                let scheduled_arrival = board_call
                    .and_then(|c| c.booked_arrival.or(c.booked_departure))
                    .map(|t| t.to_string())
                    .unwrap_or_default();

                let expected_arrival = board_call.and_then(|c| {
                    let exp = c.expected_arrival().or(c.expected_departure())?;
                    let sched = c.booked_arrival.or(c.booked_departure)?;
                    // Only show expected if different from scheduled
                    if exp != sched {
                        Some(exp.to_string())
                    } else {
                        None
                    }
                });

                // Get terminus (last call) arrival info
                let terminus_call = m.service.service.calls.last();

                let terminus_name = terminus_call
                    .map(|c| c.station_name.clone())
                    .unwrap_or_default();

                let scheduled_terminus_arrival = terminus_call
                    .and_then(|c| c.booked_arrival)
                    .map(|t| t.to_string())
                    .unwrap_or_default();

                let expected_terminus_arrival = terminus_call.and_then(|c| {
                    let exp = c.expected_arrival()?;
                    let sched = c.booked_arrival?;
                    // Only show expected if different from scheduled
                    if exp != sched {
                        Some(exp.to_string())
                    } else {
                        None
                    }
                });

                TrainMatchView {
                    service: ServiceView::from_service(&m.service.service),
                    rtt_url: rtt_search_url_default(&next_station, date, dep_time),
                    is_exact: m.confidence == MatchConfidence::Exact,
                    next_station_name,
                    scheduled_arrival,
                    expected_arrival,
                    terminus_name,
                    scheduled_terminus_arrival,
                    expected_terminus_arrival,
                }
            })
            .collect();

        let template = IdentifyResultsTemplate {
            matches: match_views,
            next_station: next_station.as_str().to_string(),
            terminus: terminus.map(|t| t.as_str().to_string()),
        };
        let html = template.render().map_err(|e| AppError::Internal {
            message: format!("Template error: {}", e),
        })?;

        Ok(Html(html).into_response())
    } else {
        // JSON response - reuse ServiceResult format
        let results: Vec<ServiceResult> = matches
            .iter()
            .map(|m| ServiceResult::from_service(&m.service.service))
            .collect();

        Ok(Json(SearchServiceResponse { services: results }).into_response())
    }
}

/// Plan a journey from current position to destination.
async fn plan_journey(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    // Parse JSON manually so we can log the body on failure
    let req: PlanJourneyRequest = serde_json::from_slice(&body).map_err(|e| {
        eprintln!("[JSON parse error] {e}");
        eprintln!("[Body] {}", String::from_utf8_lossy(&body));
        AppError::BadRequest {
            message: format!("Invalid JSON: {e}"),
        }
    })?;
    // Parse destination CRS
    let dest_crs = Crs::parse_normalized(&req.destination).map_err(|_| AppError::BadRequest {
        message: format!("Invalid destination CRS: {}", req.destination),
    })?;

    // Parse board station CRS
    let board_station =
        Crs::parse_normalized(&req.board_station).map_err(|_| AppError::BadRequest {
            message: format!("Invalid board station CRS: {}", req.board_station),
        })?;

    // Get current time info
    let now = Local::now();
    let date = now.date_naive();
    let current_mins = (now.time().hour() * 60 + now.time().minute()) as u16;

    // Find the service from the board station's departure board
    let service = find_service_by_id(&state, &req.service_id, &board_station, date, current_mins)
        .await
        .ok_or_else(|| AppError::NotFound {
            message: format!("Service {} not found or expired", req.service_id),
        })?;

    // Create the search request
    let search_request = SearchRequest::new(service.clone(), CallIndex(req.position), dest_crs);

    // Create a service provider that uses the cached Darwin client
    let provider = CachedServiceProvider {
        darwin: state.darwin.clone(),
        date,
        current_mins,
    };

    // Run the planner
    let planner = Planner::new(&provider, &state.walkable, &state.config);
    let result = planner.search(&search_request).map_err(AppError::from)?;

    // Return HTML or JSON based on Accept header
    if accepts_html(&headers) {
        let journey_views: Vec<JourneyView> = result
            .journeys
            .iter()
            .map(JourneyView::from_journey)
            .collect();

        let template = JourneyResultsTemplate {
            journeys: journey_views,
        };
        let html = template.render().map_err(|e| AppError::Internal {
            message: format!("Template error: {}", e),
        })?;

        Ok(Html(html).into_response())
    } else {
        // JSON response
        let journeys: Vec<JourneyResult> = result
            .journeys
            .iter()
            .map(JourneyResult::from_journey)
            .collect();

        Ok(Json(PlanJourneyResponse {
            journeys,
            routes_explored: result.routes_explored,
        })
        .into_response())
    }
}

/// Find a service by its Darwin ID.
///
/// Searches the board_station first (where the service was originally found),
/// then falls back to common stations if not found.
async fn find_service_by_id(
    state: &AppState,
    service_id: &str,
    board_station: &Crs,
    date: NaiveDate,
    current_mins: u16,
) -> Option<Arc<Service>> {
    // Search the board station first - this is where the service was found
    if let Ok(services) = state
        .darwin
        .get_departures_with_details(board_station, date, current_mins, 0, 120)
        .await
    {
        for s in services.iter() {
            if s.service.service_ref.darwin_id == service_id {
                return Some(Arc::new(s.service.clone()));
            }
        }
    }

    // Fallback: try common stations (in case board_station cache expired)
    let common_stations = ["PAD", "EUS", "KGX", "VIC", "WAT", "LIV", "BHM", "MAN"];

    for station in &common_stations {
        let Ok(crs) = Crs::parse(station) else {
            continue;
        };
        if &crs == board_station {
            continue; // Already searched
        }
        let Ok(services) = state
            .darwin
            .get_departures_with_details(&crs, date, current_mins, 0, 120)
            .await
        else {
            continue;
        };
        for s in services.iter() {
            if s.service.service_ref.darwin_id == service_id {
                return Some(Arc::new(s.service.clone()));
            }
        }
    }

    None
}

/// Service provider that uses the cached Darwin client.
struct CachedServiceProvider {
    darwin: Arc<crate::cache::CachedDarwinClient>,
    date: NaiveDate,
    current_mins: u16,
}

impl crate::planner::ServiceProvider for CachedServiceProvider {
    fn get_departures(
        &self,
        station: &Crs,
        after: crate::domain::RailTime,
    ) -> Result<Vec<Arc<Service>>, SearchError> {
        // This is a synchronous trait but we have async operations
        // We use block_in_place to run the async code synchronously
        // This is not ideal but works for the MVP
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let services = self
                    .darwin
                    .get_departures_with_details(station, self.date, self.current_mins, 0, 120)
                    .await
                    .map_err(|e| SearchError::FetchError {
                        station: *station,
                        message: e.to_string(),
                    })?;

                // Filter to departures after the specified time
                let filtered: Vec<Arc<Service>> = services
                    .iter()
                    .filter(|s| {
                        s.candidate
                            .expected_departure
                            .or(Some(s.candidate.scheduled_departure))
                            .is_some_and(|t| t >= after)
                    })
                    .map(|s| Arc::new(s.service.clone()))
                    .collect();

                Ok(filtered)
            })
        })
    }
}

/// Application error type.
#[derive(Debug)]
pub enum AppError {
    BadRequest { message: String },
    NotFound { message: String },
    Internal { message: String },
}

impl From<crate::darwin::DarwinError> for AppError {
    fn from(e: crate::darwin::DarwinError) -> Self {
        AppError::Internal {
            message: e.to_string(),
        }
    }
}

impl From<SearchError> for AppError {
    fn from(e: SearchError) -> Self {
        match e {
            SearchError::InvalidRequest(msg) => AppError::BadRequest { message: msg },
            _ => AppError::Internal {
                message: e.to_string(),
            },
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match &self {
            AppError::BadRequest { message } => (StatusCode::BAD_REQUEST, message.clone()),
            AppError::NotFound { message } => (StatusCode::NOT_FOUND, message.clone()),
            AppError::Internal { message } => (StatusCode::INTERNAL_SERVER_ERROR, message.clone()),
        };

        // Log errors to stderr for debugging
        eprintln!("[{status}] {message}");

        let body = Json(ErrorResponse { error: message });
        (status, body).into_response()
    }
}
