# Running Without Darwin API Access

You don't need Darwin API credentials to run and test this app! A mock mode is available with realistic sample data.

## Quick Start (Mock Mode)

```bash
cd train-server
USE_MOCK_DARWIN=true cargo run
```

The app will start on `http://127.0.0.1:3000` with sample train services.

## What Works in Mock Mode

✅ **Departure board queries** - Search for services from PAD, RDG, BRI, SWI
✅ **Service details with calling points** - Full routes for each service
✅ **Journey planning** - Find connections between mock stations
✅ **Web UI** - Full HTMX-powered interface
✅ **API endpoints** - All JSON endpoints work with mock data

## Example Usage

### Via Web UI

Visit `http://127.0.0.1:3000` and try:
- Origin: PAD (Paddington)
- Destination: BRI (Bristol)

### Via API

```bash
# Get departures from Paddington
curl "http://127.0.0.1:3000/search/service?origin=PAD"

# Get departures to a specific destination
curl "http://127.0.0.1:3000/search/service?origin=PAD&destination=BRI"
```

### Test Journey Planning

```bash
# Plan a journey (requires a service_id from the search above)
curl -X POST http://127.0.0.1:3000/journey/plan \
  -H "Content-Type: application/json" \
  -d '{
    "service_id": "pad_service_1",
    "position": 1,
    "destination": "BRI"
  }'
```

## Mock Data Coverage

The mock data includes realistic services for common routes:
- **London Paddington** → Reading → Swindon → Bristol
- **London Paddington** → Reading → Oxford
- **London Paddington** → Reading → Swindon → Cardiff
- **Bristol** → London (return services)
- **CrossCountry** services through Reading

## Limitations of Mock Mode

- ❌ No real-time updates (static data)
- ❌ Limited station coverage (only PAD, RDG, BRI, SWI)
- ❌ Station names lookup disabled (not needed for basic testing)
- ❌ Time-based filtering not implemented (returns all mock services)

## Adding More Mock Data

See `train-server/data/README.md` for instructions on adding new stations to the mock dataset.

## Switching to Real API

When you have Darwin API credentials:

1. Set environment variables:
   ```bash
   export DARWIN_USERNAME=your_username
   export DARWIN_PASSWORD=your_password
   ```

2. Run without the mock flag:
   ```bash
   cargo run
   ```

The app will automatically use the real Darwin API instead of mock data.

## Development Workflow

Mock mode is perfect for:
- Frontend development (no API dependency)
- Testing journey planning algorithm
- Writing tests for edge cases
- Demonstrating the app to others
- Working offline

Real API mode is needed for:
- Testing with live data
- Station names lookup
- Real-time updates
- Full UK station coverage
