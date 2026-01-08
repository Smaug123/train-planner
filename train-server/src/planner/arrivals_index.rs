//! Arrivals index for destination-first journey search.
//!
//! The key insight of arrivals-first search is: any valid journey must end on
//! a train that arrives at the destination. By fetching the arrivals board first,
//! we get all candidate "final trains" and their previous calling points in one
//! API call. This dramatically reduces API calls compared to forward BFS.

use std::collections::HashMap;
use std::sync::Arc;

use crate::domain::{CallIndex, Crs, RailTime, Service};

/// Information about a train that can be boarded to reach the destination.
#[derive(Debug, Clone)]
pub struct FeederInfo {
    /// The service arriving at destination.
    pub service: Arc<Service>,
    /// Index of the call where we'd board this service.
    pub board_index: CallIndex,
    /// Expected departure time from the boarding station.
    pub board_time: RailTime,
    /// Expected arrival time at destination.
    pub dest_arrival: RailTime,
}

/// Index of services arriving at destination, keyed by their calling points.
///
/// This allows O(1) lookup of "which services can I board at station X to reach
/// the destination?"
#[derive(Debug)]
pub struct ArrivalsIndex {
    /// Destination station.
    destination: Crs,

    /// All services arriving at destination in the search window.
    arriving_services: Vec<Arc<Service>>,

    /// Map from station -> services arriving at destination that call at this station.
    /// Value includes the boarding time at that station.
    feeders: HashMap<Crs, Vec<FeederInfo>>,
}

impl ArrivalsIndex {
    /// Build index from arrivals board response.
    ///
    /// # Arguments
    ///
    /// * `destination` - The destination station CRS
    /// * `arrivals` - Services arriving at the destination, with their previous calling points
    pub fn from_arrivals(destination: Crs, arrivals: Vec<Arc<Service>>) -> Self {
        let mut feeders: HashMap<Crs, Vec<FeederInfo>> = HashMap::new();

        for service in &arrivals {
            // Find the destination call in this service
            // Note: services may continue past the destination, so we can't assume last call
            let dest_call_idx = match service.calls.iter().position(|c| c.station == destination) {
                Some(idx) => idx,
                None => continue, // Service doesn't call at destination (shouldn't happen)
            };

            let dest_call = &service.calls[dest_call_idx];

            // Get arrival time at destination
            let dest_arrival = match dest_call.expected_arrival() {
                Some(t) => t,
                None => continue, // Can't determine arrival time
            };

            // Skip if destination call is cancelled
            if dest_call.is_cancelled {
                continue;
            }

            // Index all calling points BEFORE the destination
            for (idx, call) in service.calls.iter().enumerate().take(dest_call_idx) {
                // Skip cancelled calls
                if call.is_cancelled {
                    continue;
                }

                // Need departure time to board here
                let board_time = match call.expected_departure() {
                    Some(t) => t,
                    None => continue, // Can't board here (no departure time)
                };

                feeders.entry(call.station).or_default().push(FeederInfo {
                    service: service.clone(),
                    board_index: CallIndex(idx),
                    board_time,
                    dest_arrival,
                });
            }
        }

        Self {
            destination,
            arriving_services: arrivals,
            feeders,
        }
    }

    /// Get services that can be boarded at a station to reach destination.
    pub fn feeders_at(&self, station: &Crs) -> &[FeederInfo] {
        self.feeders
            .get(station)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Check if a station is a feeder station (has services going to destination).
    pub fn is_feeder(&self, station: &Crs) -> bool {
        self.feeders.contains_key(station)
    }

    /// Get all feeder stations.
    pub fn feeder_stations(&self) -> impl Iterator<Item = &Crs> {
        self.feeders.keys()
    }

    /// Get the destination station.
    pub fn destination(&self) -> &Crs {
        &self.destination
    }

    /// Get all arriving services.
    pub fn arriving_services(&self) -> &[Arc<Service>] {
        &self.arriving_services
    }

    /// Get the number of feeder stations.
    pub fn feeder_station_count(&self) -> usize {
        self.feeders.len()
    }

    /// Get the total number of feeder entries (services × stations).
    pub fn total_feeder_count(&self) -> usize {
        self.feeders.values().map(|v| v.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Call, ServiceRef};
    use chrono::NaiveDate;

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()
    }

    fn time(s: &str) -> RailTime {
        RailTime::parse_hhmm(s, date()).unwrap()
    }

    fn crs(s: &str) -> Crs {
        Crs::parse(s).unwrap()
    }

    fn make_arriving_service(
        id: &str,
        calls_data: &[(&str, &str, &str, &str)], // (crs, name, arr, dep)
    ) -> Arc<Service> {
        let calls: Vec<Call> = calls_data
            .iter()
            .map(|(station, name, arr, dep)| {
                let mut call = Call::new(crs(station), (*name).to_string());
                if !arr.is_empty() {
                    call.booked_arrival = Some(time(arr));
                }
                if !dep.is_empty() {
                    call.booked_departure = Some(time(dep));
                }
                call
            })
            .collect();

        let board_crs = calls
            .first()
            .map(|c| c.station)
            .unwrap_or_else(|| crs("XXX"));

        Arc::new(Service {
            service_ref: ServiceRef::new(id.to_string(), board_crs),
            headcode: None,
            operator: "Test".to_string(),
            operator_code: None,
            calls,
            board_station_idx: CallIndex(0),
        })
    }

    #[test]
    fn empty_arrivals() {
        let index = ArrivalsIndex::from_arrivals(crs("PAD"), vec![]);

        assert_eq!(index.destination(), &crs("PAD"));
        assert!(index.arriving_services().is_empty());
        assert_eq!(index.feeder_station_count(), 0);
    }

    #[test]
    fn single_service_indexes_all_stops() {
        // Service: SWI -> DID -> RDG -> PAD
        let service = make_arriving_service(
            "S1",
            &[
                ("SWI", "Swindon", "", "10:00"),
                ("DID", "Didcot", "10:20", "10:22"),
                ("RDG", "Reading", "10:35", "10:37"),
                ("PAD", "Paddington", "11:00", ""),
            ],
        );

        let index = ArrivalsIndex::from_arrivals(crs("PAD"), vec![service]);

        // Should have 3 feeder stations (not PAD itself)
        assert_eq!(index.feeder_station_count(), 3);
        assert!(index.is_feeder(&crs("SWI")));
        assert!(index.is_feeder(&crs("DID")));
        assert!(index.is_feeder(&crs("RDG")));
        assert!(!index.is_feeder(&crs("PAD"))); // Destination not a feeder

        // Check feeder info at Reading
        let rdg_feeders = index.feeders_at(&crs("RDG"));
        assert_eq!(rdg_feeders.len(), 1);
        assert_eq!(rdg_feeders[0].board_time, time("10:37"));
        assert_eq!(rdg_feeders[0].dest_arrival, time("11:00"));
        assert_eq!(rdg_feeders[0].board_index, CallIndex(2));
    }

    #[test]
    fn multiple_services_same_feeder_station() {
        // Two services both calling at RDG before PAD
        let service1 = make_arriving_service(
            "S1",
            &[
                ("RDG", "Reading", "", "10:00"),
                ("PAD", "Paddington", "10:30", ""),
            ],
        );
        let service2 = make_arriving_service(
            "S2",
            &[
                ("RDG", "Reading", "", "10:15"),
                ("PAD", "Paddington", "10:45", ""),
            ],
        );

        let index = ArrivalsIndex::from_arrivals(crs("PAD"), vec![service1, service2]);

        // RDG should have 2 feeders
        let rdg_feeders = index.feeders_at(&crs("RDG"));
        assert_eq!(rdg_feeders.len(), 2);

        // Check they have different times
        let times: Vec<_> = rdg_feeders.iter().map(|f| f.board_time).collect();
        assert!(times.contains(&time("10:00")));
        assert!(times.contains(&time("10:15")));
    }

    #[test]
    fn skips_stops_without_departure_time() {
        // Service where intermediate stop has arrival but no departure (set-down only)
        let mut service = make_arriving_service(
            "S1",
            &[
                ("RDG", "Reading", "", "10:00"),
                ("TWY", "Twyford", "10:10", ""), // No departure - set down only
                ("PAD", "Paddington", "10:30", ""),
            ],
        );
        // Manually ensure TWY has no departure
        Arc::make_mut(&mut service).calls[1].booked_departure = None;

        let index = ArrivalsIndex::from_arrivals(crs("PAD"), vec![service]);

        // RDG should be a feeder, TWY should not (can't board without departure)
        assert!(index.is_feeder(&crs("RDG")));
        assert!(!index.is_feeder(&crs("TWY")));
    }

    #[test]
    fn skips_cancelled_calls() {
        let mut service = make_arriving_service(
            "S1",
            &[
                ("SWI", "Swindon", "", "10:00"),
                ("RDG", "Reading", "10:30", "10:32"),
                ("PAD", "Paddington", "11:00", ""),
            ],
        );
        // Mark RDG as cancelled
        Arc::make_mut(&mut service).calls[1].is_cancelled = true;

        let index = ArrivalsIndex::from_arrivals(crs("PAD"), vec![service]);

        // SWI should be feeder, RDG should not (cancelled)
        assert!(index.is_feeder(&crs("SWI")));
        assert!(!index.is_feeder(&crs("RDG")));
    }

    #[test]
    fn feeders_at_unknown_station_returns_empty() {
        let service = make_arriving_service(
            "S1",
            &[
                ("RDG", "Reading", "", "10:00"),
                ("PAD", "Paddington", "10:30", ""),
            ],
        );

        let index = ArrivalsIndex::from_arrivals(crs("PAD"), vec![service]);

        // Unknown station returns empty slice
        let unknown = index.feeders_at(&crs("XXX"));
        assert!(unknown.is_empty());
    }

    #[test]
    fn feeder_stations_iterator() {
        let service = make_arriving_service(
            "S1",
            &[
                ("SWI", "Swindon", "", "10:00"),
                ("RDG", "Reading", "10:30", "10:32"),
                ("PAD", "Paddington", "11:00", ""),
            ],
        );

        let index = ArrivalsIndex::from_arrivals(crs("PAD"), vec![service]);

        let stations: Vec<_> = index.feeder_stations().collect();
        assert_eq!(stations.len(), 2);
        assert!(stations.contains(&&crs("SWI")));
        assert!(stations.contains(&&crs("RDG")));
    }
}
