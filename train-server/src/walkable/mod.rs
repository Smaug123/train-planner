//! Walkable connections between stations.
//!
//! Some stations are close enough to walk between, enabling connections
//! that don't appear in the rail network (e.g., London termini).
//! This module provides lookup for walkable station pairs and their durations.

use std::collections::HashMap;

use chrono::Duration;

use crate::domain::Crs;

/// A collection of walkable connections between stations.
///
/// Connections are symmetric: if you can walk from A to B, you can walk from B to A
/// in the same time.
#[derive(Debug, Clone, Default)]
pub struct WalkableConnections {
    /// Map from (from, to) to walk duration in minutes.
    /// Stored in both directions for O(1) lookup.
    connections: HashMap<(Crs, Crs), i64>,
}

impl WalkableConnections {
    /// Create an empty collection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a walkable connection between two stations.
    ///
    /// The connection is stored symmetrically (both A→B and B→A).
    pub fn add(&mut self, from: Crs, to: Crs, duration_minutes: i64) {
        self.connections.insert((from, to), duration_minutes);
        self.connections.insert((to, from), duration_minutes);
    }

    /// Get the walk duration between two stations, if walkable.
    ///
    /// Returns `None` if the stations are not walkable.
    pub fn get(&self, from: &Crs, to: &Crs) -> Option<Duration> {
        self.connections
            .get(&(*from, *to))
            .map(|mins| Duration::minutes(*mins))
    }

    /// Check if two stations are walkable.
    pub fn is_walkable(&self, from: &Crs, to: &Crs) -> bool {
        self.connections.contains_key(&(*from, *to))
    }

    /// Get all stations walkable from a given station.
    pub fn walkable_from(&self, from: &Crs) -> Vec<(Crs, Duration)> {
        self.connections
            .iter()
            .filter(|((f, _), _)| f == from)
            .map(|((_, t), mins)| (*t, Duration::minutes(*mins)))
            .collect()
    }

    /// Returns the number of walkable pairs (counting A→B and B→A as one).
    pub fn len(&self) -> usize {
        self.connections.len() / 2
    }

    /// Returns true if there are no walkable connections.
    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }

    /// Create a closure suitable for `Journey::from_legs`.
    ///
    /// # Example
    ///
    /// ```
    /// use train_server::walkable::WalkableConnections;
    /// use train_server::domain::Crs;
    ///
    /// let connections = WalkableConnections::new();
    /// let get_walk = connections.as_lookup();
    ///
    /// // Can be used with Journey::from_legs
    /// let pad = Crs::parse("PAD").unwrap();
    /// let eus = Crs::parse("EUS").unwrap();
    /// assert!(get_walk(&pad, &eus).is_none()); // No connection added
    /// ```
    pub fn as_lookup(&self) -> impl Fn(&Crs, &Crs) -> Option<Duration> + '_ {
        |from, to| self.get(from, to)
    }
}

/// Builder for creating walkable connections.
///
/// Provides a fluent API for adding connections.
#[derive(Debug, Default)]
pub struct WalkableConnectionsBuilder {
    inner: WalkableConnections,
}

impl WalkableConnectionsBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a walkable connection.
    pub fn add(mut self, from: &str, to: &str, duration_minutes: i64) -> Self {
        if let (Some(from_crs), Some(to_crs)) = (Crs::parse(from).ok(), Crs::parse(to).ok()) {
            self.inner.add(from_crs, to_crs, duration_minutes);
        }
        self
    }

    /// Build the walkable connections.
    pub fn build(self) -> WalkableConnections {
        self.inner
    }
}

/// Create a default set of London walkable connections.
///
/// These are the commonly-used walking routes between London termini
/// and nearby Underground stations.
pub fn london_connections() -> WalkableConnections {
    WalkableConnectionsBuilder::new()
        // London termini walking connections
        // Times are approximate walking times in minutes
        .add("EUS", "KGX", 5) // Euston ↔ King's Cross (same complex)
        .add("KGX", "STP", 3) // King's Cross ↔ St Pancras (adjacent)
        .add("EUS", "STP", 7) // Euston ↔ St Pancras
        .add("PAD", "PAD", 0) // Paddington (self, for completeness)
        .add("VIC", "VXH", 15) // Victoria ↔ Vauxhall (via Tube or walk)
        .add("WAT", "WLO", 5) // Waterloo ↔ Waterloo East
        .add("CHX", "LST", 20) // Charing Cross ↔ Liverpool Street (via Tube)
        .add("CST", "MOG", 8) // Cannon Street ↔ Moorgate
        .add("LST", "MOG", 10) // Liverpool Street ↔ Moorgate
        .add("FST", "CST", 5) // Fenchurch Street ↔ Cannon Street
        .add("FST", "LST", 12) // Fenchurch Street ↔ Liverpool Street
        .add("LBG", "WAT", 20) // London Bridge ↔ Waterloo (via Tube)
        .add("LBG", "CST", 15) // London Bridge ↔ Cannon Street
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crs(s: &str) -> Crs {
        Crs::parse(s).unwrap()
    }

    #[test]
    fn empty_connections() {
        let wc = WalkableConnections::new();
        assert!(wc.is_empty());
        assert_eq!(wc.len(), 0);
        assert!(wc.get(&crs("PAD"), &crs("EUS")).is_none());
    }

    #[test]
    fn add_and_lookup() {
        let mut wc = WalkableConnections::new();
        wc.add(crs("EUS"), crs("KGX"), 5);

        assert!(!wc.is_empty());
        assert_eq!(wc.len(), 1);

        // Forward lookup
        assert_eq!(wc.get(&crs("EUS"), &crs("KGX")), Some(Duration::minutes(5)));

        // Reverse lookup (symmetric)
        assert_eq!(wc.get(&crs("KGX"), &crs("EUS")), Some(Duration::minutes(5)));

        // Non-existent
        assert!(wc.get(&crs("PAD"), &crs("EUS")).is_none());
    }

    #[test]
    fn is_walkable() {
        let mut wc = WalkableConnections::new();
        wc.add(crs("EUS"), crs("KGX"), 5);

        assert!(wc.is_walkable(&crs("EUS"), &crs("KGX")));
        assert!(wc.is_walkable(&crs("KGX"), &crs("EUS")));
        assert!(!wc.is_walkable(&crs("PAD"), &crs("EUS")));
    }

    #[test]
    fn walkable_from() {
        let mut wc = WalkableConnections::new();
        wc.add(crs("KGX"), crs("EUS"), 5);
        wc.add(crs("KGX"), crs("STP"), 3);

        let from_kgx = wc.walkable_from(&crs("KGX"));
        assert_eq!(from_kgx.len(), 2);

        let from_pad = wc.walkable_from(&crs("PAD"));
        assert!(from_pad.is_empty());
    }

    #[test]
    fn builder() {
        let wc = WalkableConnectionsBuilder::new()
            .add("EUS", "KGX", 5)
            .add("KGX", "STP", 3)
            .build();

        assert_eq!(wc.len(), 2);
        assert!(wc.is_walkable(&crs("EUS"), &crs("KGX")));
        assert!(wc.is_walkable(&crs("KGX"), &crs("STP")));
    }

    #[test]
    fn builder_ignores_invalid_crs() {
        let wc = WalkableConnectionsBuilder::new()
            .add("INVALID", "KGX", 5) // Invalid CRS
            .add("EUS", "123", 5) // Invalid CRS (digits)
            .add("EUS", "KGX", 5) // Valid
            .build();

        assert_eq!(wc.len(), 1);
    }

    #[test]
    fn london_connections_exist() {
        let wc = london_connections();

        assert!(!wc.is_empty());
        assert!(wc.is_walkable(&crs("EUS"), &crs("KGX")));
        assert!(wc.is_walkable(&crs("KGX"), &crs("STP")));
        assert!(wc.is_walkable(&crs("WAT"), &crs("WLO")));
    }

    #[test]
    fn as_lookup_closure() {
        let wc = WalkableConnectionsBuilder::new()
            .add("EUS", "KGX", 5)
            .build();

        let lookup = wc.as_lookup();

        assert_eq!(lookup(&crs("EUS"), &crs("KGX")), Some(Duration::minutes(5)));
        assert!(lookup(&crs("PAD"), &crs("EUS")).is_none());
    }
}

/// Tests that demonstrate bugs in the current implementation.
/// These tests are expected to FAIL until the bugs are fixed.
#[cfg(test)]
mod bug_tests {
    use super::*;

    fn crs(s: &str) -> Crs {
        Crs::parse(s).unwrap()
    }

    /// BUG: len() is incorrect when self-connections exist.
    ///
    /// The len() method divides by 2 assuming all connections are stored
    /// twice (A→B and B→A). But self-connections (A→A) are only stored once,
    /// making the count wrong.
    ///
    /// london_connections() includes PAD→PAD as a self-connection.
    #[test]
    fn bug_len_with_self_connection() {
        let mut wc = WalkableConnections::new();

        // Add a normal connection (stored twice: A→B and B→A)
        wc.add(crs("EUS"), crs("KGX"), 5);

        // Add a self-connection (stored once: PAD→PAD)
        wc.add(crs("PAD"), crs("PAD"), 0);

        // Internal map has 3 entries: EUS→KGX, KGX→EUS, PAD→PAD
        // len() returns connections.len() / 2 = 3 / 2 = 1
        // But we added 2 logical connections!
        assert_eq!(
            wc.len(),
            2,
            "Expected 2 connections (EUS↔KGX and PAD↔PAD), but len() returns wrong value"
        );
    }

    /// BUG: london_connections() has this bug.
    ///
    /// It includes .add("PAD", "PAD", 0) which is a self-connection,
    /// causing len() to undercount.
    #[test]
    fn bug_london_connections_len() {
        let wc = london_connections();

        // Count the actual connections defined in london_connections():
        // EUS↔KGX, KGX↔STP, EUS↔STP, PAD↔PAD (self), VIC↔VXH, WAT↔WLO,
        // CHX↔LST, CST↔MOG, LST↔MOG, FST↔CST, FST↔LST, LBG↔WAT, LBG↔CST
        // = 13 pairs
        //
        // Internal storage: 12 pairs × 2 + 1 self = 25 entries
        // len() = 25 / 2 = 12 (wrong, should be 13)

        // We can verify by counting walkable_from for all stations
        let stations = ["EUS", "KGX", "STP", "PAD", "VIC", "VXH", "WAT", "WLO",
                        "CHX", "LST", "CST", "MOG", "FST", "LBG"];
        let mut total_edges = 0;
        for s in &stations {
            if let Ok(c) = Crs::parse(s) {
                total_edges += wc.walkable_from(&c).len();
            }
        }
        // Each connection is counted twice (once from each end), except self-connections
        // So: (total_edges + self_connections) / 2 = actual connections
        // With PAD→PAD: (total_edges + 1) / 2 should equal wc.len()
        // But len() uses raw division, so this will be off

        // The actual number of defined pairs is 13
        // If len() is correct, this should pass:
        assert_eq!(
            wc.len(),
            13,
            "london_connections() defines 13 pairs but len() returns wrong count"
        );
    }

    /// BUG: Adding the same connection twice overwrites silently.
    ///
    /// There's no error or deduplication tracking when adding a connection
    /// that already exists with a different duration.
    #[test]
    fn bug_duplicate_connection_overwrites() {
        let mut wc = WalkableConnections::new();

        wc.add(crs("EUS"), crs("KGX"), 5);
        wc.add(crs("EUS"), crs("KGX"), 10); // Different duration!

        // Should this be an error? Or should we keep the first/shorter?
        // Currently it silently overwrites.

        // At minimum, len() should still be correct:
        assert_eq!(wc.len(), 1, "Duplicate add should not increase len");

        // But which duration do we have? The second one overwrote the first.
        // If we wanted "keep shorter", this would fail:
        let duration = wc.get(&crs("EUS"), &crs("KGX")).unwrap();
        assert_eq!(
            duration,
            Duration::minutes(5),
            "Expected to keep the original/shorter duration, but got overwritten"
        );
    }
}
