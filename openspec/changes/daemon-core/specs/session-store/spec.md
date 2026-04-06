# Session Store Specification

## Purpose

The session store is a SQLite database that maps `(surface, conversation_id)` pairs to `claude_session_id` values. It is the daemon's only persistent state — a routing table that survives daemon restarts. It does not store conversation content; that belongs to Claude Code's own session storage in `~/.claude/projects/`.

## ADDED Requirements

### Requirement: Database Location

The session store MUST be located at `$XDG_DATA_HOME/axios-companion/sessions.db`. This places it as a sibling of the `workspace/` directory under the same `axios-companion/` parent, but outside the workspace itself.

The database MUST NOT be inside the workspace directory. The workspace is passed to claude via `--add-dir`, which would expose the database file to the agent. The session store is daemon infrastructure, not agent-visible state.

The daemon MUST create the directory and file if they do not exist. If `XDG_DATA_HOME` is not set, it MUST fall back to `~/.local/share`.

#### Scenario: Fresh install creates the database

- **Given**: The directory `$XDG_DATA_HOME/axios-companion/` exists (created by the Tier 0 wrapper's first-run scaffolding)
- **And**: No `sessions.db` file exists
- **When**: The daemon starts
- **Then**: It creates `sessions.db` with the correct schema
- **And**: The file is readable and writable by the user

#### Scenario: Database is not inside the workspace

- **Given**: The workspace is at `$XDG_DATA_HOME/axios-companion/workspace/`
- **And**: The session store is at `$XDG_DATA_HOME/axios-companion/sessions.db`
- **When**: The wrapper passes `--add-dir $XDG_DATA_HOME/axios-companion/workspace` to claude
- **Then**: Claude does not see `sessions.db` (it is outside the `workspace/` directory)

### Requirement: Schema

The session store MUST use the following schema:

```sql
CREATE TABLE schema_version (
    version INTEGER NOT NULL
);

CREATE TABLE sessions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    surface         TEXT    NOT NULL,
    conversation_id TEXT    NOT NULL,
    claude_session_id TEXT,           -- NULL until first turn completes
    status          TEXT    NOT NULL DEFAULT 'active',
    created_at      INTEGER NOT NULL, -- Unix timestamp
    last_active_at  INTEGER NOT NULL, -- Unix timestamp
    metadata        TEXT,             -- JSON blob, nullable, for future extensibility
    UNIQUE(surface, conversation_id)
);

CREATE INDEX idx_sessions_last_active ON sessions(last_active_at);
CREATE INDEX idx_sessions_surface ON sessions(surface);
```

- `claude_session_id` is NULL when a session row is created (before the first turn's `init` event is parsed) and filled after the `session_id` is captured from stream-json.
- `status` is `"active"` for sessions in use and `"archived"` for sessions the user has explicitly closed. Archiving is a future feature; daemon-core only uses `"active"`.
- `metadata` is a JSON string for extensibility. daemon-core does not write to it; openai-gateway may use it to store session policy information.

#### Scenario: Schema is created correctly

- **Given**: A fresh `sessions.db`
- **When**: The daemon runs schema creation
- **Then**: Both tables exist with the specified columns
- **And**: The unique constraint on `(surface, conversation_id)` is enforced
- **And**: The `schema_version` table contains a single row with `version = 1`

### Requirement: Migration Strategy

The daemon MUST embed schema migrations in the binary. On startup, it reads the `schema_version` table to determine the current version, then applies any pending migrations in order.

For daemon-core (schema version 1), the migration is simply: create the tables if they do not exist. Future proposals (openai-gateway, channel adapters) may add migrations for additional columns or tables.

The migration MUST be idempotent — running the same migration twice does not fail or duplicate data.

#### Scenario: Migration from version 0 to 1

- **Given**: A new `sessions.db` with no tables
- **When**: The daemon runs migrations
- **Then**: It creates the `schema_version` and `sessions` tables
- **And**: Sets `schema_version.version = 1`

#### Scenario: Migration is a no-op when current

- **Given**: An existing `sessions.db` with `schema_version.version = 1`
- **When**: The daemon runs migrations
- **Then**: No schema changes are made
- **And**: The daemon proceeds to start normally

### Requirement: CRUD Operations

The session store MUST support the following operations:

**create_session(surface, conversation_id) → session_id:**

Insert a new row with `claude_session_id = NULL`, `status = "active"`, `created_at` and `last_active_at` set to the current Unix timestamp. Returns the auto-generated `id`. Fails with a constraint error if `(surface, conversation_id)` already exists.

**lookup_session(surface, conversation_id) → Option\<Session\>:**

Return the session row for the given pair, or None if not found.

**set_claude_session_id(id, claude_session_id):**

Update the `claude_session_id` for the given row. Called after the first turn's `init` event is parsed.

**touch_session(id):**

Update `last_active_at` to the current Unix timestamp. Called at the start of each turn.

**list_sessions() → Vec\<Session\>:**

Return all sessions, ordered by `last_active_at` descending.

**list_by_surface(surface) → Vec\<Session\>:**

Return all sessions for a given surface, ordered by `last_active_at` descending.

#### Scenario: Session lifecycle

- **Given**: No session exists for `("dbus", "conv-1")`
- **When**: `create_session("dbus", "conv-1")` is called
- **Then**: A new row is created with `claude_session_id = NULL` and `status = "active"`
- **When**: The first turn completes and session-id `"abc-123"` is captured
- **And**: `set_claude_session_id(id, "abc-123")` is called
- **Then**: The row now has `claude_session_id = "abc-123"`
- **When**: A second turn arrives and `touch_session(id)` is called
- **Then**: `last_active_at` is updated to the current time
- **When**: `lookup_session("dbus", "conv-1")` is called
- **Then**: It returns the session with `claude_session_id = "abc-123"`

#### Scenario: Duplicate session is rejected

- **Given**: A session exists for `("dbus", "conv-1")`
- **When**: `create_session("dbus", "conv-1")` is called again
- **Then**: The operation fails with a constraint error
- **And**: The existing session is not modified

### Requirement: WAL Mode

The session store MUST be opened with `PRAGMA journal_mode=WAL` to allow concurrent reads during writes. The dispatcher serializes writes per-session, so write contention is low, but WAL ensures that `ListSessions` and `GetStatus` D-Bus methods can read without blocking in-flight turns.

#### Scenario: Read during write

- **Given**: A turn is in progress and the dispatcher is updating `last_active_at`
- **When**: A D-Bus client calls `ListSessions()`
- **Then**: The read completes without blocking
- **And**: Returns a consistent snapshot of the sessions table

### Requirement: Daemon Restart Recovery

On startup, the daemon MUST open the existing `sessions.db` and make all prior session mappings available. In-flight turns from a prior daemon instance are lost (the claude subprocesses died with the daemon), but the session mappings remain valid — the next message to any session will resume via `--resume <claude_session_id>`.

The daemon MUST NOT clear or rebuild the session store on startup. The store is append-only in practice (new sessions are created, existing sessions are updated, nothing is deleted in daemon-core).

#### Scenario: Daemon restarts with existing sessions

- **Given**: The daemon previously processed messages creating sessions `conv-1` and `conv-2`
- **And**: The daemon was stopped (cleanly or by crash)
- **When**: The daemon starts again
- **Then**: `lookup_session("dbus", "conv-1")` returns the existing session with its `claude_session_id`
- **And**: The next message to `conv-1` resumes the claude session

#### Scenario: Daemon crash does not corrupt the store

- **Given**: The daemon crashes (SIGKILL) during a write to the session store
- **When**: The daemon restarts and opens the database
- **Then**: The database is intact (WAL recovery handles incomplete writes)
- **And**: The daemon starts normally
