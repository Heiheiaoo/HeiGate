# HeiGate Desktop Runtime

The desktop runtime is a local-only gateway. It does not require an account,
PostgreSQL, Redis, cloud sync, or the SaaS setup wizard.

## Build

From the repository root:

```bash
make build-desktop
```

The output is `dist/desktop/heigate-desktop`.

## Run

```bash
DESKTOP_ONLY=true DATA_DIR="$PWD/desktop-data" ./dist/desktop/heigate-desktop
```

Or use the command-line switch:

```bash
DATA_DIR="$PWD/desktop-data" ./dist/desktop/heigate-desktop -desktop
```

On macOS, open `dist/desktop/HeiGate.app` by double-clicking it. The
application creates its own native WebKit window and starts the local service
in the background; it does not open Safari or Chrome. The local gateway uses:

- `http://127.0.0.1:<port>/v1/messages`
- `http://127.0.0.1:<port>/v1/chat/completions`
- `http://127.0.0.1:<port>/v1/responses`

Set `DESKTOP_SERVER_PORT` when port 8080 is already occupied. Export the
channel configuration from the desktop UI before moving it to another
computer; the exported JSON is intended for manual local migration.
