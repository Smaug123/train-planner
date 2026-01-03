//! Askama templates for the web frontend.

use askama::Template;

use crate::domain::{Journey, Segment, Service};

// ============================================================================
// Page Templates (extend base.html)
// ============================================================================

/// Home page with search form.
#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexTemplate;

/// About page.
#[derive(Template)]
#[template(path = "about.html")]
pub struct AboutTemplate;

/// Error page.
#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorTemplate {
    pub title: String,
    pub message: String,
    pub details: Option<String>,
}

// ============================================================================
// Fragment Templates (AJAX responses, no base.html)
// ============================================================================

/// Service list fragment (search results).
#[derive(Template)]
#[template(path = "service_list.html")]
pub struct ServiceListTemplate {
    pub services: Vec<ServiceView>,
}

/// Journey results fragment.
#[derive(Template)]
#[template(path = "journey_results.html")]
pub struct JourneyResultsTemplate {
    pub journeys: Vec<JourneyView>,
}

/// Train identification results fragment.
#[derive(Template)]
#[template(path = "identify_results.html")]
pub struct IdentifyResultsTemplate {
    pub matches: Vec<TrainMatchView>,
    pub next_station: String,
    pub terminus: Option<String>,
}

// ============================================================================
// View Models (for templates)
// ============================================================================

/// Service view model for templates.
#[derive(Debug, Clone)]
pub struct ServiceView {
    pub service_id: String,
    pub headcode: Option<String>,
    pub operator: String,
    pub destination: String,
    pub scheduled_departure: String,
    pub expected_departure: Option<String>,
    pub platform: Option<String>,
    pub is_cancelled: bool,
    pub calls: Vec<CallView>,
}

impl ServiceView {
    /// The time to display (expected if available, else scheduled).
    pub fn display_time(&self) -> &str {
        self.expected_departure
            .as_deref()
            .unwrap_or(&self.scheduled_departure)
    }

    /// Whether the service is delayed.
    pub fn is_delayed(&self) -> bool {
        self.expected_departure
            .as_ref()
            .is_some_and(|exp| exp != &self.scheduled_departure)
    }

    /// Create from a domain Service.
    pub fn from_service(service: &Service) -> Self {
        let calls: Vec<CallView> = service
            .calls
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let scheduled = c
                    .booked_departure
                    .or(c.booked_arrival)
                    .map(|t| t.to_string());
                let expected = c
                    .expected_departure()
                    .or(c.expected_arrival())
                    .map(|t| t.to_string());

                // Has subsequent stops if not the last call
                let has_subsequent = i < service.calls.len() - 1;

                CallView {
                    index: i,
                    crs: c.station.as_str().to_string(),
                    name: c.station_name.clone(),
                    scheduled_time: scheduled.clone().unwrap_or_default(),
                    expected_time: expected.clone(),
                    platform: c.platform.clone(),
                    is_cancelled: c.is_cancelled,
                    has_subsequent_stops: has_subsequent && !c.is_cancelled,
                }
            })
            .collect();

        let destination = service
            .calls
            .last()
            .map(|c| c.station_name.clone())
            .unwrap_or_default();

        let board_call = service.calls.get(service.board_station_idx.0);

        let scheduled_departure = board_call
            .and_then(|c| c.booked_departure)
            .map(|t| t.to_string())
            .unwrap_or_default();

        let expected_departure = board_call
            .and_then(|c| c.expected_departure())
            .map(|t| t.to_string());

        let platform = board_call.and_then(|c| c.platform.clone());

        let is_cancelled = board_call.is_some_and(|c| c.is_cancelled);

        Self {
            service_id: service.service_ref.darwin_id.clone(),
            headcode: service.headcode.as_ref().map(|h| h.to_string()),
            operator: service.operator.clone(),
            destination,
            scheduled_departure,
            expected_departure,
            platform,
            is_cancelled,
            calls,
        }
    }
}

/// Calling point view model.
#[derive(Debug, Clone)]
pub struct CallView {
    pub index: usize,
    pub crs: String,
    pub name: String,
    pub scheduled_time: String,
    pub expected_time: Option<String>,
    pub platform: Option<String>,
    pub is_cancelled: bool,
    pub has_subsequent_stops: bool,
}

impl CallView {
    /// The time to display.
    pub fn display_time(&self) -> &str {
        self.expected_time
            .as_deref()
            .unwrap_or(&self.scheduled_time)
    }

    /// Whether this call is delayed.
    pub fn is_delayed(&self) -> bool {
        self.expected_time
            .as_ref()
            .is_some_and(|exp| exp != &self.scheduled_time)
    }
}

/// Journey view model for templates.
#[derive(Debug, Clone)]
pub struct JourneyView {
    pub departure_time: String,
    pub arrival_time: String,
    pub duration_display: String,
    pub changes: usize,
    pub segments: Vec<SegmentView>,
}

impl JourneyView {
    /// Create from a domain Journey.
    pub fn from_journey(journey: &Journey) -> Self {
        let segments: Vec<SegmentView> = journey
            .segments()
            .iter()
            .map(SegmentView::from_segment)
            .collect();

        let duration = journey.total_duration();
        let hours = duration.num_hours();
        let mins = duration.num_minutes() % 60;

        let duration_display = if hours > 0 {
            format!("{}h {}m", hours, mins)
        } else {
            format!("{}m", mins)
        };

        Self {
            departure_time: journey.departure_time().to_string(),
            arrival_time: journey.arrival_time().to_string(),
            duration_display,
            changes: journey.change_count(),
            segments,
        }
    }
}

/// Segment view model (train or walk).
#[derive(Debug, Clone)]
pub enum SegmentView {
    Train(LegView),
    Walk(WalkView),
}

impl SegmentView {
    /// Create from a domain Segment.
    pub fn from_segment(segment: &Segment) -> Self {
        match segment {
            Segment::Train(leg) => SegmentView::Train(LegView::from_leg(leg)),
            Segment::Walk(walk) => SegmentView::Walk(WalkView::from_walk(walk)),
        }
    }
}

/// Train leg view model.
#[derive(Debug, Clone)]
pub struct LegView {
    pub operator: String,
    pub headcode: Option<String>,
    pub origin: StationView,
    pub destination: StationView,
    pub stops: usize,
}

impl LegView {
    /// Create from a domain Leg.
    pub fn from_leg(leg: &crate::domain::Leg) -> Self {
        let origin = StationView {
            crs: leg.board_call().station.as_str().to_string(),
            name: leg.board_call().station_name.clone(),
            time: leg
                .board_call()
                .expected_departure()
                .map(|t| t.to_string())
                .unwrap_or_default(),
            platform: leg.board_call().platform.clone(),
        };

        let destination = StationView {
            crs: leg.alight_call().station.as_str().to_string(),
            name: leg.alight_call().station_name.clone(),
            time: leg
                .alight_call()
                .expected_arrival()
                .map(|t| t.to_string())
                .unwrap_or_default(),
            platform: leg.alight_call().platform.clone(),
        };

        // Count intermediate stops
        let stops = leg.intermediate_stop_count();

        Self {
            operator: leg.service().operator.clone(),
            headcode: leg.service().headcode.as_ref().map(|h| h.to_string()),
            origin,
            destination,
            stops,
        }
    }
}

/// Walking segment view model.
#[derive(Debug, Clone)]
pub struct WalkView {
    pub from_crs: String,
    pub from_name: String,
    pub to_crs: String,
    pub to_name: String,
    pub duration_mins: i64,
}

impl WalkView {
    /// Create from a domain Walk.
    pub fn from_walk(walk: &crate::domain::Walk) -> Self {
        Self {
            from_crs: walk.from.as_str().to_string(),
            // Note: Walk doesn't store names, so we use CRS as fallback
            // A proper implementation would use a station index lookup
            from_name: walk.from.as_str().to_string(),
            to_crs: walk.to.as_str().to_string(),
            to_name: walk.to.as_str().to_string(),
            duration_mins: walk.duration.num_minutes(),
        }
    }
}

/// Station view model for display.
#[derive(Debug, Clone)]
pub struct StationView {
    pub crs: String,
    pub name: String,
    pub time: String,
    pub platform: Option<String>,
}

/// Train match view model for identification results.
#[derive(Debug, Clone)]
pub struct TrainMatchView {
    /// The matched service
    pub service: ServiceView,
    /// RTT search URL for verification
    pub rtt_url: String,
    /// Whether this is an exact match (both next station and terminus)
    pub is_exact: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_view_display_time_scheduled() {
        let view = ServiceView {
            service_id: "123".into(),
            headcode: None,
            operator: "Test".into(),
            destination: "Dest".into(),
            scheduled_departure: "10:00".into(),
            expected_departure: None,
            platform: None,
            is_cancelled: false,
            calls: vec![],
        };

        assert_eq!(view.display_time(), "10:00");
        assert!(!view.is_delayed());
    }

    #[test]
    fn service_view_display_time_delayed() {
        let view = ServiceView {
            service_id: "123".into(),
            headcode: None,
            operator: "Test".into(),
            destination: "Dest".into(),
            scheduled_departure: "10:00".into(),
            expected_departure: Some("10:15".into()),
            platform: None,
            is_cancelled: false,
            calls: vec![],
        };

        assert_eq!(view.display_time(), "10:15");
        assert!(view.is_delayed());
    }

    #[test]
    fn service_view_on_time() {
        let view = ServiceView {
            service_id: "123".into(),
            headcode: None,
            operator: "Test".into(),
            destination: "Dest".into(),
            scheduled_departure: "10:00".into(),
            expected_departure: Some("10:00".into()),
            platform: None,
            is_cancelled: false,
            calls: vec![],
        };

        assert!(!view.is_delayed());
    }

    #[test]
    fn call_view_delayed() {
        let view = CallView {
            index: 0,
            crs: "PAD".into(),
            name: "Paddington".into(),
            scheduled_time: "10:00".into(),
            expected_time: Some("10:05".into()),
            platform: None,
            is_cancelled: false,
            has_subsequent_stops: true,
        };

        assert!(view.is_delayed());
        assert_eq!(view.display_time(), "10:05");
    }
}
