# Mock Data for Development

This directory contains sample Darwin API responses for testing and development without requiring real API credentials.

## Using Mock Mode

To run the app with mock data instead of the real Darwin API:

```bash
USE_MOCK_DARWIN=true cargo run
```

The app will load JSON files from `data/mock_boards/` and serve them as if they were live API responses.

## Available Mock Stations

Currently available mock boards:
- **PAD** (London Paddington) - Services to Bristol, Oxford, Cardiff
- **RDG** (Reading) - Services from London continuing to Bristol, Oxford, Brighton
- **BRI** (Bristol Temple Meads) - Services arriving from London, departing to London and Cardiff
- **SWI** (Swindon) - Services from London to Bristol/Cardiff, and returns to London

## Mock Data Format

Each file is a Darwin `StationBoardWithDetails` JSON response, named `{CRS}.json`.

Example: `PAD.json` contains the departure board for London Paddington.

The JSON structure matches the real Darwin API exactly, including:
- Service IDs (ephemeral identifiers)
- Scheduled and estimated times
- Calling points (previous and subsequent)
- Platform information
- Operator details

## Adding New Mock Stations

To add a new station:

1. Create a new JSON file: `data/mock_boards/{CRS}.json`
2. Follow the structure of existing files (see `PAD.json` as a template)
3. Ensure times form valid chronological sequences
4. Include both `previousCallingPoints` and `subsequentCallingPoints` where appropriate

The mock client will automatically load any `.json` files it finds in `data/mock_boards/`.

## Testing Journey Planning

The mock data includes interconnected services for testing journey planning:

**Example: PAD → BRI via RDG**
1. User boards `pad_service_1` at PAD (14:15 departure)
2. Service calls at RDG (14:40 arrival, 14:42 departure)
3. Service continues to BRI (15:45 arrival)

**Example: Change at RDG**
1. User is on `pad_service_2` to OXF, currently at RDG
2. Could change to `rdg_service_3` (CrossCountry to Brighton)

## Notes

- Mock data uses static times (2026-01-03) - services don't update in real-time
- Service IDs in mock data are descriptive (e.g., `pad_service_1`) rather than the real ephemeral IDs
- All mock services show "On time" status for simplicity
- The mock client ignores time_offset and time_window parameters (returns all services)
