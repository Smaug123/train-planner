# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build and Test Commands

Standard Cargo Rust project.
Use Clippy and `cargo fmt`.

## Architecture

This is a Rust web application for planning train journey connections. The user specifies their current train and destination; the app finds onward journey options using the Darwin LDB (Live Departure Board) API.

### Module Structure

- **`domain/`** - Core validated types. All types enforce invariants at construction time:
  - `Crs` - 3-letter station codes
  - `Headcode` - Train identity (digit, letter, two digits like "1A23")
  - `AtocCode` - Operator codes (two uppercase letters)
  - `RailTime` - Date-aware time for handling overnight services
  - `Call`, `CallIndex` - Station calls within a service
  - `Service`, `ServiceRef`, `ServiceCandidate` - Train service representations
  - `Leg`, `Journey`, `Segment`, `Walk` - Journey building blocks

- **`darwin/`** - Darwin API integration:
  - `types.rs` - API response DTOs
  - `convert.rs` - DTO → domain type conversions
  - `client.rs` - HTTP client with rate limiting

- **`planner/`** - BFS journey-finding algorithm:
  - `search.rs` - Core BFS with pruning
  - `rank.rs` - Journey ranking/deduplication
  - `config.rs` - Search configuration

- **`walkable/`** - Connections between nearby stations (e.g., KGX ↔ STP)

- **`cache.rs`** - Moka cache for Darwin responses (60s TTL)

- **`web/`** - Axum handlers (HTMX-powered, no JS required)

### Key Design Decisions

**Darwin service IDs are ephemeral** - only valid while a service appears on a departure board (~2 min after departure). No stable service URLs possible; caching is board-based, not service-based.

**Time handling with rollover detection** - Darwin returns "HH:MM" without dates. Uses 6-hour threshold: if a calling point time appears >6 hours earlier than the previous, increment the date by one day.

**Walkable connections are directed edges** - not equivalence classes. KGX↔STP walkable doesn't imply KGX↔EUS walkable.

## Environment Variables

```bash
DARWIN_USERNAME=<username from Rail Data Marketplace>
DARWIN_PASSWORD=<password from Rail Data Marketplace>
LISTEN_ADDR=127.0.0.1:3000
```
