//! Darwin API response DTOs.
//!
//! These types map directly to the Darwin LDB JSON API responses.
//! They use `Option` liberally because Darwin omits fields rather than
//! sending null values in many cases.

use serde::Deserialize;

/// Response from `GetDepBoardWithDetails` or `GetArrDepBoardWithDetails`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StationBoardWithDetails {
    /// When this response was generated (ISO 8601 datetime).
    pub generated_at: String,

    /// Human-readable name of the station.
    pub location_name: String,

    /// CRS code of the station.
    pub crs: String,

    /// Train services at this station.
    pub train_services: Option<Vec<ServiceItemWithCallingPoints>>,

    /// Bus replacement services.
    pub bus_services: Option<Vec<ServiceItemWithCallingPoints>>,

    /// Ferry services (rare).
    pub ferry_services: Option<Vec<ServiceItemWithCallingPoints>>,

    /// Whether platform information is available at this station.
    pub platform_available: Option<bool>,

    /// Whether services are available (false during disruption).
    pub are_services_available: Option<bool>,

    /// Network Rail communication messages.
    pub nrcc_messages: Option<Vec<NrccMessage>>,
}

/// A service on the departure board, including calling points.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceItemWithCallingPoints {
    /// Ephemeral Darwin service ID. Only valid while on departure board.
    #[serde(rename = "serviceID")]
    pub service_id: String,

    /// Retail Service ID (headcode-like, e.g., "GW123400").
    pub rsid: Option<String>,

    /// Scheduled time of arrival at this station.
    pub sta: Option<String>,

    /// Estimated time of arrival at this station.
    pub eta: Option<String>,

    /// Scheduled time of departure from this station.
    pub std: Option<String>,

    /// Estimated time of departure from this station.
    /// May be "On time", "Delayed", "Cancelled", or a time like "10:15".
    pub etd: Option<String>,

    /// Platform number/letter.
    pub platform: Option<String>,

    /// Train operating company name.
    pub operator: Option<String>,

    /// Train operating company ATOC code.
    pub operator_code: Option<String>,

    /// Whether this service is cancelled.
    pub is_cancelled: Option<bool>,

    /// Service type (train, bus, ferry).
    pub service_type: Option<ServiceType>,

    /// Train length in coaches.
    pub length: Option<i32>,

    /// Origin station(s).
    pub origin: Option<Vec<ServiceLocation>>,

    /// Destination station(s).
    pub destination: Option<Vec<ServiceLocation>>,

    /// Previous calling points (stations already visited).
    pub previous_calling_points: Option<Vec<ArrayOfCallingPoints>>,

    /// Subsequent calling points (stations still to visit).
    pub subsequent_calling_points: Option<Vec<ArrayOfCallingPoints>>,

    /// Reason for cancellation (if cancelled).
    pub cancel_reason: Option<String>,

    /// Reason for delay (if delayed).
    pub delay_reason: Option<String>,
}

/// Response from `GetServiceDetails`.
///
/// Note: This endpoint only works while the service is on a departure board
/// (~2 minutes after expected departure).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceDetails {
    /// When this response was generated.
    pub generated_at: String,

    /// Station name where this detail request originated.
    pub location_name: String,

    /// CRS code of the originating station.
    pub crs: String,

    /// Train operating company name.
    pub operator: Option<String>,

    /// Train operating company ATOC code.
    pub operator_code: Option<String>,

    /// Retail Service ID.
    pub rsid: Option<String>,

    /// Whether the service is cancelled.
    pub is_cancelled: Option<bool>,

    /// Cancellation reason.
    pub cancel_reason: Option<String>,

    /// Delay reason.
    pub delay_reason: Option<String>,

    /// Platform at the board station.
    pub platform: Option<String>,

    /// Scheduled arrival at board station.
    pub sta: Option<String>,

    /// Estimated arrival at board station.
    pub eta: Option<String>,

    /// Actual arrival at board station.
    pub ata: Option<String>,

    /// Scheduled departure from board station.
    pub std: Option<String>,

    /// Estimated departure from board station.
    pub etd: Option<String>,

    /// Actual departure from board station.
    pub atd: Option<String>,

    /// Service type.
    pub service_type: Option<ServiceType>,

    /// Train length.
    pub length: Option<i32>,

    /// Previous calling points.
    pub previous_calling_points: Option<Vec<ArrayOfCallingPoints>>,

    /// Subsequent calling points.
    pub subsequent_calling_points: Option<Vec<ArrayOfCallingPoints>>,
}

/// Wrapper for a list of calling points.
///
/// Darwin wraps calling points in this structure to support split/join services,
/// where multiple arrays represent different portions of a train.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArrayOfCallingPoints {
    /// The calling points in this portion.
    pub calling_point: Vec<CallingPoint>,

    /// Service type for this portion (usually matches parent).
    pub service_type: Option<ServiceType>,

    /// Whether a change of service is required at the split point.
    pub service_change_required: Option<bool>,

    /// Whether the associated service is cancelled (for joins).
    pub assoc_is_cancelled: Option<bool>,
}

/// A single calling point (station stop).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallingPoint {
    /// Human-readable station name.
    pub location_name: String,

    /// CRS code of the station.
    pub crs: String,

    /// Scheduled time (arrival for previous, departure for subsequent).
    pub st: Option<String>,

    /// Estimated time.
    pub et: Option<String>,

    /// Actual time (only present after the train has called).
    pub at: Option<String>,

    /// Whether this call is cancelled.
    pub is_cancelled: Option<bool>,

    /// Train length at this stop (may change due to coupling/uncoupling).
    pub length: Option<i32>,

    /// Cancellation reason for this stop.
    pub cancel_reason: Option<String>,

    /// Delay reason at this stop.
    pub delay_reason: Option<String>,
}

/// Origin or destination location.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceLocation {
    /// Human-readable station name.
    pub location_name: String,

    /// CRS code.
    pub crs: String,

    /// "via" text (e.g., "via Bristol Parkway").
    pub via: Option<String>,

    /// Future change information.
    pub future_change_to: Option<String>,
}

/// Service type enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceType {
    Train,
    Bus,
    Ferry,
}

/// Network Rail communication message.
#[derive(Debug, Clone, Deserialize)]
pub struct NrccMessage {
    /// The message content (may contain HTML).
    #[serde(rename = "Value")]
    pub value: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_station_board() {
        let json = r#"{
            "generatedAt": "2024-03-15T10:30:00Z",
            "locationName": "London Paddington",
            "crs": "PAD",
            "platformAvailable": true,
            "areServicesAvailable": true,
            "trainServices": [
                {
                    "serviceID": "abc123",
                    "std": "10:45",
                    "etd": "On time",
                    "platform": "1",
                    "operator": "Great Western Railway",
                    "operatorCode": "GW",
                    "destination": [
                        {"locationName": "Bristol Temple Meads", "crs": "BRI"}
                    ],
                    "subsequentCallingPoints": [
                        {
                            "callingPoint": [
                                {"locationName": "Reading", "crs": "RDG", "st": "11:10", "et": "On time"},
                                {"locationName": "Bristol Temple Meads", "crs": "BRI", "st": "12:00", "et": "On time"}
                            ]
                        }
                    ]
                }
            ]
        }"#;

        let board: StationBoardWithDetails = serde_json::from_str(json).unwrap();

        assert_eq!(board.location_name, "London Paddington");
        assert_eq!(board.crs, "PAD");
        assert!(board.platform_available.unwrap());

        let services = board.train_services.unwrap();
        assert_eq!(services.len(), 1);

        let service = &services[0];
        assert_eq!(service.service_id, "abc123");
        assert_eq!(service.std.as_deref(), Some("10:45"));
        assert_eq!(service.etd.as_deref(), Some("On time"));
        assert_eq!(service.platform.as_deref(), Some("1"));

        let dest = service.destination.as_ref().unwrap();
        assert_eq!(dest[0].location_name, "Bristol Temple Meads");
        assert_eq!(dest[0].crs, "BRI");

        let subsequent = service.subsequent_calling_points.as_ref().unwrap();
        let calls = &subsequent[0].calling_point;
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].location_name, "Reading");
        assert_eq!(calls[0].crs, "RDG");
    }

    #[test]
    fn deserialize_calling_point() {
        let json = r#"{
            "locationName": "Reading",
            "crs": "RDG",
            "st": "10:25",
            "et": "10:28",
            "isCancelled": false
        }"#;

        let cp: CallingPoint = serde_json::from_str(json).unwrap();

        assert_eq!(cp.location_name, "Reading");
        assert_eq!(cp.crs, "RDG");
        assert_eq!(cp.st.as_deref(), Some("10:25"));
        assert_eq!(cp.et.as_deref(), Some("10:28"));
        assert_eq!(cp.is_cancelled, Some(false));
    }

    #[test]
    fn deserialize_cancelled_service() {
        let json = r#"{
            "serviceID": "xyz789",
            "std": "14:00",
            "etd": "Cancelled",
            "isCancelled": true,
            "cancelReason": "A fault with the signalling system",
            "destination": [
                {"locationName": "Oxford", "crs": "OXF"}
            ]
        }"#;

        let service: ServiceItemWithCallingPoints = serde_json::from_str(json).unwrap();

        assert!(service.is_cancelled.unwrap());
        assert_eq!(service.etd.as_deref(), Some("Cancelled"));
        assert!(service.cancel_reason.is_some());
    }

    #[test]
    fn deserialize_service_with_actual_time() {
        let json = r#"{
            "locationName": "Swindon",
            "crs": "SWI",
            "st": "10:52",
            "at": "10:54"
        }"#;

        let cp: CallingPoint = serde_json::from_str(json).unwrap();

        assert_eq!(cp.st.as_deref(), Some("10:52"));
        assert_eq!(cp.at.as_deref(), Some("10:54"));
        assert!(cp.et.is_none()); // No estimate once actual is known
    }

    #[test]
    fn deserialize_service_type() {
        assert_eq!(
            serde_json::from_str::<ServiceType>(r#""train""#).unwrap(),
            ServiceType::Train
        );
        assert_eq!(
            serde_json::from_str::<ServiceType>(r#""bus""#).unwrap(),
            ServiceType::Bus
        );
        assert_eq!(
            serde_json::from_str::<ServiceType>(r#""ferry""#).unwrap(),
            ServiceType::Ferry
        );
    }

    #[test]
    fn deserialize_service_details() {
        let json = r#"{
            "generatedAt": "2024-03-15T10:30:00Z",
            "locationName": "Reading",
            "crs": "RDG",
            "operator": "Great Western Railway",
            "operatorCode": "GW",
            "platform": "7",
            "sta": "10:25",
            "eta": "On time",
            "std": "10:27",
            "etd": "On time",
            "previousCallingPoints": [
                {
                    "callingPoint": [
                        {"locationName": "London Paddington", "crs": "PAD", "st": "10:00", "at": "10:00"}
                    ]
                }
            ],
            "subsequentCallingPoints": [
                {
                    "callingPoint": [
                        {"locationName": "Swindon", "crs": "SWI", "st": "10:52"},
                        {"locationName": "Bristol Temple Meads", "crs": "BRI", "st": "11:30"}
                    ]
                }
            ]
        }"#;

        let details: ServiceDetails = serde_json::from_str(json).unwrap();

        assert_eq!(details.location_name, "Reading");
        assert_eq!(details.operator.as_deref(), Some("Great Western Railway"));

        let prev = details.previous_calling_points.as_ref().unwrap();
        assert_eq!(prev[0].calling_point[0].location_name, "London Paddington");

        let subseq = details.subsequent_calling_points.as_ref().unwrap();
        assert_eq!(subseq[0].calling_point.len(), 2);
    }
}
