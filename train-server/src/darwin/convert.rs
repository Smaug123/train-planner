//! Conversion from Darwin DTOs to domain types.
//!
//! This module handles the transformation of raw Darwin API responses into
//! our validated domain types, including time parsing with rollover detection.

use chrono::NaiveDate;

use crate::domain::{
    AtocCode, Call, CallIndex, Crs, Headcode, RailTime, Service, ServiceCandidate, ServiceRef,
    parse_time_sequence, parse_time_sequence_reverse,
};

use super::types::{CallingPoint, ServiceItemWithCallingPoints, StationBoardWithDetails};

/// Error during DTO to domain conversion.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ConversionError {
    /// Failed to parse a CRS code
    #[error("invalid CRS code: {0}")]
    InvalidCrs(String),

    /// Failed to parse a time string
    #[error("invalid time: {0}")]
    InvalidTime(String),

    /// Missing required field
    #[error("missing required field: {0}")]
    MissingField(&'static str),

    /// Invalid service structure
    #[error("invalid service: {0}")]
    InvalidService(&'static str),
}

/// Result of converting a Darwin service item.
pub struct ConvertedService {
    /// Summary info for display on departure boards
    pub candidate: ServiceCandidate,
    /// Full service with calling points
    pub service: Service,
}

/// Convert a departure board response to domain types.
///
/// Returns converted services paired with candidates for display.
pub fn convert_station_board(
    board: &StationBoardWithDetails,
    board_date: NaiveDate,
) -> Result<Vec<ConvertedService>, ConversionError> {
    let board_crs =
        Crs::parse(&board.crs).map_err(|_| ConversionError::InvalidCrs(board.crs.clone()))?;

    let train_services = board.train_services.as_deref().unwrap_or(&[]);

    let mut results = Vec::with_capacity(train_services.len());

    for service_item in train_services {
        match convert_service_item(service_item, &board_crs, &board.location_name, board_date) {
            Ok(converted) => results.push(converted),
            Err(e) => {
                // Log and skip invalid services rather than failing the whole board
                // In production, we'd use proper logging here
                eprintln!(
                    "Warning: skipping service {}: {}",
                    service_item.service_id, e
                );
            }
        }
    }

    Ok(results)
}

/// Convert a single service item to domain types.
pub fn convert_service_item(
    item: &ServiceItemWithCallingPoints,
    board_crs: &Crs,
    board_station_name: &str,
    board_date: NaiveDate,
) -> Result<ConvertedService, ConversionError> {
    // Parse the service reference
    let service_ref = ServiceRef::new(item.service_id.clone(), *board_crs);

    // Parse headcode from RSID if available (format: "GW123400" -> "1234")
    let headcode = item.rsid.as_ref().and_then(|rsid| {
        // RSID format is typically "XX1234YY" where XX is operator, 1234 is headcode
        if rsid.len() >= 6 {
            Headcode::parse(&rsid[2..6])
        } else {
            None
        }
    });

    // Parse operator code
    let operator_code = item
        .operator_code
        .as_ref()
        .and_then(|c| AtocCode::parse(c).ok());

    // Parse scheduled departure time at board station
    let scheduled_departure = item
        .std
        .as_ref()
        .ok_or(ConversionError::MissingField("std (scheduled departure)"))?;
    let scheduled_departure = RailTime::parse_hhmm(scheduled_departure, board_date)
        .map_err(|_| ConversionError::InvalidTime(scheduled_departure.clone()))?;

    // Parse expected departure (may be "On time", "Delayed", "Cancelled", or a time)
    let expected_departure = parse_expected_time(item.etd.as_deref(), &scheduled_departure);

    // Parse destination info
    let (destination, destination_crs) = parse_destination(item);

    // Build the ServiceCandidate
    let candidate = ServiceCandidate {
        service_ref: service_ref.clone(),
        headcode,
        scheduled_departure,
        expected_departure,
        destination,
        destination_crs,
        operator: item.operator.clone().unwrap_or_default(),
        operator_code,
        platform: item.platform.clone(),
        is_cancelled: item.is_cancelled.unwrap_or(false),
    };

    // Build the full Service with calling points
    let (calls, board_station_idx) = build_calls(item, board_crs, board_station_name, board_date)?;

    let service = Service {
        service_ref,
        headcode,
        operator: item.operator.clone().unwrap_or_default(),
        operator_code,
        calls,
        board_station_idx,
    };

    Ok(ConvertedService { candidate, service })
}

/// Parse an expected time field, which may be a time or a status string.
fn parse_expected_time(etd: Option<&str>, scheduled: &RailTime) -> Option<RailTime> {
    let etd = etd?;

    // Check for status strings
    match etd {
        "On time" => Some(*scheduled),
        "Cancelled" | "Delayed" | "" => None,
        time_str => {
            // Try to parse as time
            RailTime::parse_hhmm(time_str, scheduled.date()).ok()
        }
    }
}

/// Extract destination name and CRS from service item.
fn parse_destination(item: &ServiceItemWithCallingPoints) -> (String, Option<Crs>) {
    let destinations = item.destination.as_ref();

    match destinations {
        Some(dests) if !dests.is_empty() => {
            let first = &dests[0];
            let crs = Crs::parse(&first.crs).ok();

            // Build destination string, handling multiple destinations
            let name = if dests.len() == 1 {
                first.location_name.clone()
            } else {
                // Multiple destinations (split service)
                dests
                    .iter()
                    .map(|d| d.location_name.as_str())
                    .collect::<Vec<_>>()
                    .join(" & ")
            };

            (name, crs)
        }
        _ => ("Unknown".to_string(), None),
    }
}

/// Build the calls list and determine board station index.
fn build_calls(
    item: &ServiceItemWithCallingPoints,
    board_crs: &Crs,
    board_station_name: &str,
    board_date: NaiveDate,
) -> Result<(Vec<Call>, CallIndex), ConversionError> {
    let mut calls = Vec::new();

    // 1. Parse previous calling points (if any)
    let previous_calls = parse_previous_calling_points(item, board_date)?;

    // 2. Create the board station call
    let board_call = create_board_station_call(item, board_crs, board_station_name, board_date)?;

    // 3. Parse subsequent calling points (if any)
    // Pass the board station's scheduled departure for midnight rollover detection
    // Fall back to sta if std is not available (e.g., at a terminus)
    let anchor_time = item.std.as_deref().or(item.sta.as_deref());
    let subsequent_calls = parse_subsequent_calling_points(item, anchor_time, board_date)?;

    // 4. Merge: previous + board + subsequent
    calls.extend(previous_calls);
    let board_station_idx = CallIndex(calls.len());
    calls.push(board_call);
    calls.extend(subsequent_calls);

    Ok((calls, board_station_idx))
}

/// Parse previous calling points into domain Calls.
fn parse_previous_calling_points(
    item: &ServiceItemWithCallingPoints,
    board_date: NaiveDate,
) -> Result<Vec<Call>, ConversionError> {
    let previous = match &item.previous_calling_points {
        Some(arrays) if !arrays.is_empty() => &arrays[0].calling_point,
        _ => return Ok(Vec::new()),
    };

    if previous.is_empty() {
        return Ok(Vec::new());
    }

    // Previous calling points are in forward chronological order (origin first).
    // We need to:
    // 1. Reverse them to get reverse chronological order (most recent first)
    // 2. Parse with parse_time_sequence_reverse from board_date
    // 3. Reverse the result back to forward chronological order

    let reversed: Vec<&CallingPoint> = previous.iter().rev().collect();

    // Extract times for parsing
    let times: Vec<Option<&str>> = reversed.iter().map(|cp| cp.st.as_deref()).collect();

    let parsed_times = parse_time_sequence_reverse(&times, board_date)
        .map_err(|e| ConversionError::InvalidTime(e.to_string()))?;

    // Build calls in reverse order (which we'll reverse again)
    // Previous calling points are never the final destination
    let mut calls: Vec<Call> = reversed
        .iter()
        .zip(parsed_times.iter())
        .map(|(cp, time)| calling_point_to_call(cp, *time, false))
        .collect::<Result<Vec<_>, _>>()?;

    // Reverse back to forward chronological order
    calls.reverse();

    Ok(calls)
}

/// Parse subsequent calling points into domain Calls.
///
/// Takes the board station's scheduled departure time to properly handle
/// overnight services that cross midnight.
fn parse_subsequent_calling_points(
    item: &ServiceItemWithCallingPoints,
    board_std: Option<&str>,
    board_date: NaiveDate,
) -> Result<Vec<Call>, ConversionError> {
    let subsequent = match &item.subsequent_calling_points {
        Some(arrays) if !arrays.is_empty() => &arrays[0].calling_point,
        _ => return Ok(Vec::new()),
    };

    if subsequent.is_empty() {
        return Ok(Vec::new());
    }

    // Include the board station departure time as first element to detect midnight rollover.
    // For example: board at 23:30, first subsequent at 00:15 -> should be next day.
    let mut times: Vec<Option<&str>> = Vec::with_capacity(subsequent.len() + 1);
    times.push(board_std);
    times.extend(subsequent.iter().map(|cp| cp.st.as_deref()));

    let parsed_times = parse_time_sequence(&times, board_date)
        .map_err(|e| ConversionError::InvalidTime(e.to_string()))?;

    // Skip the first parsed time (board station) and use the rest
    let count = subsequent.len();
    subsequent
        .iter()
        .zip(parsed_times.iter().skip(1))
        .enumerate()
        .map(|(idx, (cp, time))| {
            let is_final_destination = idx == count - 1;
            calling_point_to_call(cp, *time, is_final_destination)
        })
        .collect()
}

/// Convert a CallingPoint DTO to a domain Call.
///
/// `is_final_destination` indicates whether this is the last stop (terminus),
/// in which case the time represents arrival, not departure.
fn calling_point_to_call(
    cp: &CallingPoint,
    scheduled_time: Option<RailTime>,
    is_final_destination: bool,
) -> Result<Call, ConversionError> {
    let station = Crs::parse(&cp.crs).map_err(|_| ConversionError::InvalidCrs(cp.crs.clone()))?;

    let mut call = Call::new(station, cp.location_name.clone());

    // Set times based on whether this is arrival or departure
    // For calling points, `st` is the scheduled time (departure for intermediate,
    // arrival for terminus), and `et`/`at` is the expected/actual time.
    if let Some(st) = scheduled_time {
        if is_final_destination {
            // Final destination: time is arrival
            call.booked_arrival = Some(st);

            // Parse realtime (et or at)
            let realtime = cp.at.as_deref().or(cp.et.as_deref());
            if let Some(rt_str) = realtime
                && let Ok(rt) = RailTime::parse_hhmm(rt_str, st.date())
            {
                call.realtime_arrival = Some(rt);
            }
        } else {
            // Intermediate stop: time is departure
            call.booked_departure = Some(st);

            // Parse realtime (et or at)
            let realtime = cp.at.as_deref().or(cp.et.as_deref());
            if let Some(rt_str) = realtime
                && let Ok(rt) = RailTime::parse_hhmm(rt_str, st.date())
            {
                call.realtime_departure = Some(rt);
            }
        }
    }

    call.is_cancelled = cp.is_cancelled.unwrap_or(false);

    Ok(call)
}

/// Create the Call for the board station itself.
fn create_board_station_call(
    item: &ServiceItemWithCallingPoints,
    board_crs: &Crs,
    board_station_name: &str,
    board_date: NaiveDate,
) -> Result<Call, ConversionError> {
    let mut call = Call::new(*board_crs, board_station_name.to_string());

    // Parse arrival time (sta/eta) if present
    if let Some(sta) = &item.sta
        && let Ok(t) = RailTime::parse_hhmm(sta, board_date)
    {
        call.booked_arrival = Some(t);

        // Parse expected arrival
        if let Some(rt) = parse_expected_time(item.eta.as_deref(), &t) {
            call.realtime_arrival = Some(rt);
        }
    }

    // Parse departure time (std/etd)
    if let Some(std) = &item.std
        && let Ok(t) = RailTime::parse_hhmm(std, board_date)
    {
        call.booked_departure = Some(t);

        // Parse expected departure
        if let Some(rt) = parse_expected_time(item.etd.as_deref(), &t) {
            call.realtime_departure = Some(rt);
        }
    }

    call.platform = item.platform.clone();
    call.is_cancelled = item.is_cancelled.unwrap_or(false);

    Ok(call)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::darwin::types::{ArrayOfCallingPoints, ServiceLocation};

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()
    }

    fn make_calling_point(name: &str, crs: &str, st: &str) -> CallingPoint {
        CallingPoint {
            location_name: name.to_string(),
            crs: crs.to_string(),
            st: Some(st.to_string()),
            et: None,
            at: None,
            is_cancelled: None,
            length: None,
            cancel_reason: None,
            delay_reason: None,
        }
    }

    fn make_service_item(
        service_id: &str,
        std: &str,
        destination_crs: &str,
        destination_name: &str,
    ) -> ServiceItemWithCallingPoints {
        ServiceItemWithCallingPoints {
            service_id: service_id.to_string(),
            rsid: None,
            sta: None,
            eta: None,
            std: Some(std.to_string()),
            etd: Some("On time".to_string()),
            platform: Some("1".to_string()),
            operator: Some("Great Western Railway".to_string()),
            operator_code: Some("GW".to_string()),
            is_cancelled: Some(false),
            service_type: None,
            length: None,
            origin: None,
            destination: Some(vec![ServiceLocation {
                location_name: destination_name.to_string(),
                crs: destination_crs.to_string(),
                via: None,
                future_change_to: None,
            }]),
            previous_calling_points: None,
            subsequent_calling_points: None,
            cancel_reason: None,
            delay_reason: None,
        }
    }

    #[test]
    fn convert_simple_service() {
        let item = make_service_item("ABC123", "10:00", "BRI", "Bristol Temple Meads");

        let board_crs = Crs::parse("PAD").unwrap();
        let result = convert_service_item(&item, &board_crs, "London Paddington", date()).unwrap();

        assert_eq!(result.candidate.service_ref.darwin_id, "ABC123");
        assert_eq!(result.candidate.scheduled_departure.to_string(), "10:00");
        assert_eq!(result.candidate.destination, "Bristol Temple Meads");
        assert_eq!(
            result.candidate.destination_crs,
            Some(Crs::parse("BRI").unwrap())
        );
        assert_eq!(result.candidate.platform, Some("1".to_string()));
        assert!(!result.candidate.is_cancelled);

        // Service should have just the board station call
        assert_eq!(result.service.calls.len(), 1);
        assert_eq!(result.service.board_station_idx, CallIndex(0));
    }

    #[test]
    fn convert_service_with_subsequent_calls() {
        let mut item = make_service_item("ABC123", "10:00", "BRI", "Bristol Temple Meads");
        item.subsequent_calling_points = Some(vec![ArrayOfCallingPoints {
            calling_point: vec![
                make_calling_point("Reading", "RDG", "10:25"),
                make_calling_point("Swindon", "SWI", "10:52"),
                make_calling_point("Bristol Temple Meads", "BRI", "11:30"),
            ],
            service_type: None,
            service_change_required: None,
            assoc_is_cancelled: None,
        }]);

        let board_crs = Crs::parse("PAD").unwrap();
        let result = convert_service_item(&item, &board_crs, "London Paddington", date()).unwrap();

        // 1 board station + 3 subsequent = 4 calls
        assert_eq!(result.service.calls.len(), 4);
        assert_eq!(result.service.board_station_idx, CallIndex(0));

        // Check order
        assert_eq!(result.service.calls[0].station, Crs::parse("PAD").unwrap());
        assert_eq!(result.service.calls[1].station, Crs::parse("RDG").unwrap());
        assert_eq!(result.service.calls[2].station, Crs::parse("SWI").unwrap());
        assert_eq!(result.service.calls[3].station, Crs::parse("BRI").unwrap());
    }

    #[test]
    fn convert_service_with_previous_calls() {
        let mut item = make_service_item("ABC123", "10:27", "BRI", "Bristol Temple Meads");
        item.previous_calling_points = Some(vec![ArrayOfCallingPoints {
            calling_point: vec![make_calling_point("London Paddington", "PAD", "10:00")],
            service_type: None,
            service_change_required: None,
            assoc_is_cancelled: None,
        }]);

        let board_crs = Crs::parse("RDG").unwrap();
        let result = convert_service_item(&item, &board_crs, "Reading", date()).unwrap();

        // 1 previous + 1 board station = 2 calls
        assert_eq!(result.service.calls.len(), 2);
        assert_eq!(result.service.board_station_idx, CallIndex(1));

        // Check order
        assert_eq!(result.service.calls[0].station, Crs::parse("PAD").unwrap());
        assert_eq!(result.service.calls[1].station, Crs::parse("RDG").unwrap());
    }

    #[test]
    fn convert_service_with_both_previous_and_subsequent() {
        let mut item = make_service_item("ABC123", "10:27", "BRI", "Bristol Temple Meads");
        item.sta = Some("10:25".to_string());
        item.eta = Some("On time".to_string());
        item.previous_calling_points = Some(vec![ArrayOfCallingPoints {
            calling_point: vec![make_calling_point("London Paddington", "PAD", "10:00")],
            service_type: None,
            service_change_required: None,
            assoc_is_cancelled: None,
        }]);
        item.subsequent_calling_points = Some(vec![ArrayOfCallingPoints {
            calling_point: vec![
                make_calling_point("Swindon", "SWI", "10:52"),
                make_calling_point("Bristol Temple Meads", "BRI", "11:30"),
            ],
            service_type: None,
            service_change_required: None,
            assoc_is_cancelled: None,
        }]);

        let board_crs = Crs::parse("RDG").unwrap();
        let result = convert_service_item(&item, &board_crs, "Reading", date()).unwrap();

        // 1 previous + 1 board + 2 subsequent = 4 calls
        assert_eq!(result.service.calls.len(), 4);
        assert_eq!(result.service.board_station_idx, CallIndex(1));

        // Check order
        assert_eq!(result.service.calls[0].station, Crs::parse("PAD").unwrap());
        assert_eq!(result.service.calls[1].station, Crs::parse("RDG").unwrap());
        assert_eq!(result.service.calls[2].station, Crs::parse("SWI").unwrap());
        assert_eq!(result.service.calls[3].station, Crs::parse("BRI").unwrap());

        // Check board station has both arrival and departure
        let board_call = &result.service.calls[1];
        assert!(board_call.booked_arrival.is_some());
        assert!(board_call.booked_departure.is_some());
    }

    #[test]
    fn convert_cancelled_service() {
        let mut item = make_service_item("ABC123", "10:00", "BRI", "Bristol Temple Meads");
        item.is_cancelled = Some(true);
        item.etd = Some("Cancelled".to_string());

        let board_crs = Crs::parse("PAD").unwrap();
        let result = convert_service_item(&item, &board_crs, "London Paddington", date()).unwrap();

        assert!(result.candidate.is_cancelled);
        assert!(result.candidate.expected_departure.is_none());
    }

    #[test]
    fn convert_delayed_service() {
        let mut item = make_service_item("ABC123", "10:00", "BRI", "Bristol Temple Meads");
        item.etd = Some("10:15".to_string());

        let board_crs = Crs::parse("PAD").unwrap();
        let result = convert_service_item(&item, &board_crs, "London Paddington", date()).unwrap();

        assert_eq!(
            result.candidate.expected_departure.unwrap().to_string(),
            "10:15"
        );
        assert!(result.candidate.is_delayed());
    }

    #[test]
    fn parse_expected_time_on_time() {
        let scheduled = RailTime::parse_hhmm("10:00", date()).unwrap();
        let result = parse_expected_time(Some("On time"), &scheduled);
        assert_eq!(result, Some(scheduled));
    }

    #[test]
    fn parse_expected_time_cancelled() {
        let scheduled = RailTime::parse_hhmm("10:00", date()).unwrap();
        let result = parse_expected_time(Some("Cancelled"), &scheduled);
        assert!(result.is_none());
    }

    #[test]
    fn parse_expected_time_delayed_string() {
        let scheduled = RailTime::parse_hhmm("10:00", date()).unwrap();
        let result = parse_expected_time(Some("Delayed"), &scheduled);
        assert!(result.is_none());
    }

    #[test]
    fn parse_expected_time_actual_time() {
        let scheduled = RailTime::parse_hhmm("10:00", date()).unwrap();
        let result = parse_expected_time(Some("10:15"), &scheduled);
        assert_eq!(result.unwrap().to_string(), "10:15");
    }

    #[test]
    fn parse_destination_single() {
        let item = make_service_item("ABC", "10:00", "BRI", "Bristol Temple Meads");
        let (name, crs) = parse_destination(&item);

        assert_eq!(name, "Bristol Temple Meads");
        assert_eq!(crs, Some(Crs::parse("BRI").unwrap()));
    }

    #[test]
    fn parse_destination_multiple() {
        let mut item = make_service_item("ABC", "10:00", "BRI", "Bristol Temple Meads");
        item.destination = Some(vec![
            ServiceLocation {
                location_name: "Bristol Temple Meads".to_string(),
                crs: "BRI".to_string(),
                via: None,
                future_change_to: None,
            },
            ServiceLocation {
                location_name: "Cardiff Central".to_string(),
                crs: "CDF".to_string(),
                via: None,
                future_change_to: None,
            },
        ]);

        let (name, crs) = parse_destination(&item);

        assert_eq!(name, "Bristol Temple Meads & Cardiff Central");
        // First destination's CRS
        assert_eq!(crs, Some(Crs::parse("BRI").unwrap()));
    }

    #[test]
    fn convert_overnight_service_subsequent() {
        // Service departing at 23:30, arriving after midnight
        let mut item = make_service_item("NIGHT", "23:30", "EDI", "Edinburgh");
        item.subsequent_calling_points = Some(vec![ArrayOfCallingPoints {
            calling_point: vec![
                make_calling_point("York", "YRK", "00:15"),
                make_calling_point("Edinburgh", "EDI", "04:30"),
            ],
            service_type: None,
            service_change_required: None,
            assoc_is_cancelled: None,
        }]);

        let board_crs = Crs::parse("KGX").unwrap();
        let result = convert_service_item(&item, &board_crs, "London Kings Cross", date()).unwrap();

        // Check that times roll over correctly
        let board_call = &result.service.calls[0];
        assert_eq!(board_call.booked_departure.unwrap().date(), date());

        // York is intermediate: has departure time
        let york_call = &result.service.calls[1];
        let next_day = NaiveDate::from_ymd_opt(2024, 3, 16).unwrap();
        assert_eq!(york_call.booked_departure.unwrap().date(), next_day);

        // Edinburgh is final destination: has arrival time (not departure)
        let edi_call = &result.service.calls[2];
        assert_eq!(edi_call.booked_arrival.unwrap().date(), next_day);
    }

    #[test]
    fn convert_overnight_service_previous() {
        // Boarding at 00:30, service started previous day
        let mut item = make_service_item("NIGHT", "00:35", "EDI", "Edinburgh");
        item.sta = Some("00:30".to_string());
        item.previous_calling_points = Some(vec![ArrayOfCallingPoints {
            calling_point: vec![
                make_calling_point("London Kings Cross", "KGX", "23:30"),
                make_calling_point("Peterborough", "PBO", "00:15"),
            ],
            service_type: None,
            service_change_required: None,
            assoc_is_cancelled: None,
        }]);

        let board_date = NaiveDate::from_ymd_opt(2024, 3, 16).unwrap();
        let board_crs = Crs::parse("YRK").unwrap();
        let result = convert_service_item(&item, &board_crs, "York", board_date).unwrap();

        // KGX should be on previous day
        let kgx_call = &result.service.calls[0];
        let prev_day = NaiveDate::from_ymd_opt(2024, 3, 15).unwrap();
        assert_eq!(kgx_call.booked_departure.unwrap().date(), prev_day);

        // PBO should be on board_date
        let pbo_call = &result.service.calls[1];
        assert_eq!(pbo_call.booked_departure.unwrap().date(), board_date);

        // York (board station) should be on board_date
        let board_call = &result.service.calls[2];
        assert_eq!(board_call.booked_departure.unwrap().date(), board_date);
    }

    #[test]
    fn headcode_from_rsid() {
        // RSID format: 2-letter operator + 4-char headcode + 2 additional digits
        // Headcode format: digit-letter-digit-digit (e.g., "1A23")
        let mut item = make_service_item("ABC123", "10:00", "BRI", "Bristol Temple Meads");
        item.rsid = Some("GW1A2300".to_string());

        let board_crs = Crs::parse("PAD").unwrap();
        let result = convert_service_item(&item, &board_crs, "London Paddington", date()).unwrap();

        assert_eq!(
            result.candidate.headcode,
            Some(Headcode::parse("1A23").unwrap())
        );
    }

    #[test]
    fn headcode_from_rsid_invalid_format() {
        // RSID "GW123400" has "1234" which is all digits, not a valid headcode
        let mut item = make_service_item("ABC123", "10:00", "BRI", "Bristol Temple Meads");
        item.rsid = Some("GW123400".to_string());

        let board_crs = Crs::parse("PAD").unwrap();
        let result = convert_service_item(&item, &board_crs, "London Paddington", date()).unwrap();

        // Should be None since "1234" doesn't match headcode format
        assert_eq!(result.candidate.headcode, None);
    }
}

/// Tests for fixed behavior that was previously buggy.
#[cfg(test)]
mod fixed_behavior_tests {
    use super::*;
    use crate::darwin::types::ArrayOfCallingPoints;

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()
    }

    fn make_calling_point(name: &str, crs: &str, st: &str) -> CallingPoint {
        CallingPoint {
            location_name: name.to_string(),
            crs: crs.to_string(),
            st: Some(st.to_string()),
            et: None,
            at: None,
            is_cancelled: None,
            length: None,
            cancel_reason: None,
            delay_reason: None,
        }
    }

    /// FIXED: Midnight rollover uses anchor time for detection.
    ///
    /// The parse_subsequent_calling_points function uses the board station's
    /// departure (or arrival as fallback) to detect midnight crossings.
    /// This test verifies that late-night services correctly roll over to the next day.
    #[test]
    fn subsequent_rollover_detects_midnight_crossing() {
        use crate::darwin::types::ServiceLocation;

        // Create an overnight service departing late night
        let item = ServiceItemWithCallingPoints {
            service_id: "NIGHT".to_string(),
            rsid: None,
            sta: Some("23:45".to_string()),
            eta: Some("On time".to_string()),
            std: Some("23:50".to_string()), // Departure at 23:50
            etd: Some("On time".to_string()),
            platform: Some("1".to_string()),
            operator: Some("Test".to_string()),
            operator_code: None,
            is_cancelled: Some(false),
            service_type: None,
            length: None,
            origin: None,
            destination: Some(vec![ServiceLocation {
                location_name: "Edinburgh".to_string(),
                crs: "EDI".to_string(),
                via: None,
                future_change_to: None,
            }]),
            previous_calling_points: None,
            subsequent_calling_points: Some(vec![ArrayOfCallingPoints {
                calling_point: vec![
                    // First subsequent is after midnight
                    make_calling_point("Edinburgh", "EDI", "00:30"),
                ],
                service_type: None,
                service_change_required: None,
                assoc_is_cancelled: None,
            }]),
            cancel_reason: None,
            delay_reason: None,
        };

        // Board at York at 23:50
        let board_crs = Crs::parse("YRK").unwrap();
        let result = convert_service_item(&item, &board_crs, "York", date());

        assert!(result.is_ok(), "Conversion should succeed");

        let service = result.unwrap().service;

        // Verify board station is on the original date
        let board_call = &service.calls[0];
        assert_eq!(board_call.booked_departure.unwrap().date(), date());

        // Edinburgh at 00:30 should be on the next day (March 16)
        let edi_call = service.calls.last().unwrap();
        let expected_date = NaiveDate::from_ymd_opt(2024, 3, 16).unwrap();
        assert_eq!(
            edi_call.booked_arrival.unwrap().date(),
            expected_date,
            "Edinburgh at 00:30 should be on next day after midnight rollover"
        );
    }

    /// FIXED: Final destination now has arrival time, not departure.
    ///
    /// The calling_point_to_call function now correctly sets booked_arrival
    /// for the final destination instead of booked_departure.
    #[test]
    fn final_destination_has_arrival_not_departure() {
        use crate::darwin::types::ServiceLocation;

        let item = ServiceItemWithCallingPoints {
            service_id: "ABC".to_string(),
            rsid: None,
            sta: None,
            eta: None,
            std: Some("10:00".to_string()),
            etd: Some("On time".to_string()),
            platform: Some("1".to_string()),
            operator: Some("Test".to_string()),
            operator_code: None,
            is_cancelled: Some(false),
            service_type: None,
            length: None,
            origin: None,
            destination: Some(vec![ServiceLocation {
                location_name: "Bristol".to_string(),
                crs: "BRI".to_string(),
                via: None,
                future_change_to: None,
            }]),
            previous_calling_points: None,
            subsequent_calling_points: Some(vec![ArrayOfCallingPoints {
                calling_point: vec![
                    make_calling_point("Reading", "RDG", "10:30"),
                    make_calling_point("Bristol", "BRI", "11:00"), // This is arrival
                ],
                service_type: None,
                service_change_required: None,
                assoc_is_cancelled: None,
            }]),
            cancel_reason: None,
            delay_reason: None,
        };

        let board_crs = Crs::parse("PAD").unwrap();
        let result = convert_service_item(&item, &board_crs, "Paddington", date()).unwrap();

        // Check intermediate stop (Reading) has departure
        let reading_call = &result.service.calls[1];
        assert!(
            reading_call.booked_departure.is_some(),
            "Intermediate stop should have booked_departure"
        );
        assert!(
            reading_call.booked_arrival.is_none(),
            "Intermediate stop should not have booked_arrival"
        );

        // Check final destination (Bristol) has arrival
        let bristol_call = result.service.calls.last().unwrap();
        assert!(
            bristol_call.booked_arrival.is_some(),
            "Final destination should have booked_arrival set"
        );
        assert!(
            bristol_call.booked_departure.is_none(),
            "Final destination should NOT have booked_departure set"
        );
    }
}
