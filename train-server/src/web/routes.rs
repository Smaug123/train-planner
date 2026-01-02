//! HTTP route handlers.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use chrono::{Local, NaiveDate, Timelike};

use crate::domain::{CallIndex, Crs, Service};
use crate::planner::{Planner, SearchError, SearchRequest};

use super::dto::*;
use super::state::AppState;

/// Create the application router.
pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .route("/search/service", get(search_service))
        .route("/journey/plan", post(plan_journey))
        .with_state(state)
}

/// Health check endpoint.
async fn health() -> &'static str {
    "ok"
}

/// Index page (placeholder).
async fn index() -> &'static str {
    "Train Journey Planner API"
}

/// Search for services from a station.
async fn search_service(
    State(state): State<AppState>,
    Query(req): Query<SearchServiceRequest>,
) -> Result<Json<SearchServiceResponse>, AppError> {
    // Parse origin CRS
    let origin_crs = Crs::parse(&req.origin).map_err(|_| AppError::BadRequest {
        message: format!("Invalid origin CRS: {}", req.origin),
    })?;

    // Parse optional destination CRS
    let dest_crs = req
        .destination
        .as_ref()
        .map(|d| Crs::parse(d))
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

    // Convert to response
    let results: Vec<ServiceResult> = services
        .iter()
        .map(|s| ServiceResult::from_service(&s.service))
        .collect();

    Ok(Json(SearchServiceResponse { services: results }))
}

/// Plan a journey from current position to destination.
async fn plan_journey(
    State(state): State<AppState>,
    Json(req): Json<PlanJourneyRequest>,
) -> Result<Json<PlanJourneyResponse>, AppError> {
    // Parse destination CRS
    let dest_crs = Crs::parse(&req.destination).map_err(|_| AppError::BadRequest {
        message: format!("Invalid destination CRS: {}", req.destination),
    })?;

    // Get current time info
    let now = Local::now();
    let date = now.date_naive();
    let current_mins = (now.time().hour() * 60 + now.time().minute()) as u16;

    // We need to find the service from the cache
    // This is a limitation - the service_id is ephemeral and we need to search for it
    // In practice, this would be called immediately after search_service
    // so the service should still be in cache

    // For now, search all cached stations for the service ID
    // This is inefficient but works for the MVP
    let service = find_service_by_id(&state, &req.service_id, date, current_mins)
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

    // Convert to response
    let journeys: Vec<JourneyResult> = result
        .journeys
        .iter()
        .map(JourneyResult::from_journey)
        .collect();

    Ok(Json(PlanJourneyResponse {
        journeys,
        routes_explored: result.routes_explored,
    }))
}

/// Find a service by its Darwin ID.
async fn find_service_by_id(
    state: &AppState,
    service_id: &str,
    date: NaiveDate,
    current_mins: u16,
) -> Option<Arc<Service>> {
    // Get cached boards - we try a few common stations
    // This is a hack - in production we'd track which station the service came from
    let common_stations = ["PAD", "EUS", "KGX", "VIC", "WAT", "LIV", "BHM", "MAN"];

    for station in &common_stations {
        let Ok(crs) = Crs::parse(station) else {
            continue;
        };
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
        let (status, message) = match self {
            AppError::BadRequest { message } => (StatusCode::BAD_REQUEST, message),
            AppError::NotFound { message } => (StatusCode::NOT_FOUND, message),
            AppError::Internal { message } => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };

        let body = Json(ErrorResponse { error: message });
        (status, body).into_response()
    }
}
