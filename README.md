# Five-In-A-Row Server (Axum + PostgreSQL)

## Prereqs

- Rust toolchain
- PostgreSQL

## Setup

1) Create database, e.g. `five_in_a_row`
2) Copy env:

```bash
cp .env.example .env
```

3) Fill in `.env`:

- `DATABASE_URL` must point to your Postgres
- `JWT_SECRET` must be a long random string

3) Run:

```bash
cargo run
```

The server will auto-run SQLx migrations on startup.

## Endpoints

- `GET /healthz`
- `POST /api/v1/auth/register`
- `POST /api/v1/auth/login`
- `POST /api/v1/auth/refresh`
- `GET /api/v1/auth/me`
- `POST /api/v1/auth/logout`
- `GET /ws` (WebSocket; requires `accessToken` query or `Authorization: Bearer ...`)
