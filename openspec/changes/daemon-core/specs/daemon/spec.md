# Daemon Lifecycle Specification

## Purpose

The `companion-core` binary is a Rust async daemon that runs as a `systemd --user` service. This specification defines how the daemon starts, shuts down, recovers from errors, and reports health. It does not cover what the daemon does with messages — that is the dispatcher spec's concern.

## ADDED Requirements

### Requirement: Rust Async Binary With Tokio Runtime

The `companion-core` binary MUST be a Rust binary using the tokio async runtime. The binary MUST be buildable via Nix (`rustPlatform.buildRustPackage` or equivalent) and produce a single static binary with no runtime dependencies beyond libc and system libraries (dbus, sqlite).

#### Scenario: Binary builds and runs

- **Given**: The Nix flake has a `companion-core` package defined
- **When**: A user runs `nix build .#companion-core`
- **Then**: The result is a single `companion-core` binary in `$out/bin/`
- **And**: The binary starts, logs its version, and enters the event loop

### Requirement: Systemd User Service With Type=notify

The daemon MUST run as a `systemd --user` service using `Type=notify` for readiness signaling. The service unit MUST be managed by the home-manager module.

The unit MUST specify:

- `Type=notify` — the daemon signals readiness via `sd_notify`
- `Restart=on-failure` — automatic restart on unexpected exit
- `RestartSec=5` — five-second delay between restarts
- `Environment=` — any daemon configuration set by the home-manager module (e.g., session store path, log level)

The unit MUST NOT specify:

- `ExecStartPre` — all initialization happens inside the binary
- `WatchdogSec` — deferred; the daemon does not implement watchdog pings in the initial version

#### Scenario: Service installs and starts

- **Given**: A user has `services.axios-companion.daemon.enable = true` in their home-manager config
- **When**: They run `home-manager switch`
- **Then**: The `companion-core.service` unit is installed in `~/.config/systemd/user/`
- **And**: `systemctl --user start companion-core` starts the daemon
- **And**: `systemctl --user status companion-core` shows `active (running)`

#### Scenario: Service restarts on crash

- **Given**: The `companion-core` daemon is running
- **When**: The daemon process crashes (exit code non-zero)
- **Then**: systemd restarts it after 5 seconds
- **And**: The daemon resumes with all session mappings intact from the SQLite store

### Requirement: Startup Sequence

The daemon MUST execute its startup sequence in this order:

1. Initialize structured logging via `tracing` to the systemd journal
2. Open (or create) the SQLite session store and run pending migrations
3. Initialize the dispatcher
4. Acquire the D-Bus well-known name `org.axios.Companion` on the session bus
5. Signal readiness via `sd_notify(READY=1)`
6. Enter the tokio event loop

If any step fails, the daemon MUST log the error and exit with a non-zero status. It MUST NOT signal readiness if initialization is incomplete.

#### Scenario: Clean startup

- **Given**: No other process owns `org.axios.Companion` on the session bus
- **And**: The session store path is writable
- **When**: The daemon starts
- **Then**: It acquires the D-Bus name, signals ready, and accepts method calls

#### Scenario: D-Bus name already taken

- **Given**: Another process owns `org.axios.Companion` on the session bus
- **When**: The daemon attempts to start
- **Then**: It logs an error explaining the name conflict
- **And**: It exits with a non-zero status
- **And**: systemd records the failure

#### Scenario: Session store path is not writable

- **Given**: The directory for `sessions.db` does not exist and cannot be created
- **When**: The daemon attempts to start
- **Then**: It logs an error explaining the path issue
- **And**: It exits with a non-zero status

### Requirement: Graceful Shutdown

On receiving SIGTERM (or equivalent systemd stop), the daemon MUST:

1. Stop accepting new D-Bus method calls (release the well-known name or stop processing)
2. Wait for all in-flight `claude` subprocess turns to complete (with a configurable timeout, default 120 seconds)
3. Close the D-Bus connection
4. Close the SQLite connection
5. Exit with status 0

If in-flight turns do not complete within the timeout, the daemon MUST send SIGTERM to the remaining `claude` subprocesses and exit.

#### Scenario: Clean shutdown with no in-flight turns

- **Given**: The daemon is running with no active claude subprocesses
- **When**: `systemctl --user stop companion-core` is issued
- **Then**: The daemon exits within 2 seconds with status 0

#### Scenario: Shutdown with in-flight turn

- **Given**: The daemon is running with one claude subprocess in progress
- **When**: `systemctl --user stop companion-core` is issued
- **Then**: The daemon waits for the subprocess to complete
- **And**: Then exits with status 0
- **And**: The D-Bus client that initiated the turn receives the response or an error

#### Scenario: Shutdown timeout exceeded

- **Given**: The daemon is running with a claude subprocess that does not exit within 120 seconds
- **When**: `systemctl --user stop companion-core` is issued
- **Then**: The daemon sends SIGTERM to the subprocess after the timeout
- **And**: Then exits

### Requirement: Claude Subprocess Failure Does Not Crash The Daemon

If a `claude` subprocess exits with a non-zero status, crashes, or produces unparseable output, the daemon MUST:

1. Log the failure with the subprocess exit code and any captured stderr
2. Emit a `ResponseError` signal to the requesting surface
3. Mark the turn as failed in the session store (update `last_active_at`, status remains `active`)
4. Continue accepting new requests on all surfaces

The daemon MUST NOT crash, panic, or enter a degraded state due to a subprocess failure.

#### Scenario: Claude exits non-zero

- **Given**: The daemon dispatches a turn via `companion -p "prompt" --output-format stream-json --verbose`
- **And**: The claude subprocess exits with status 1
- **When**: The next message arrives for the same session
- **Then**: The daemon handles it normally, spawning a new subprocess
- **And**: The prior failure is logged but does not affect the new turn

#### Scenario: Claude produces invalid JSON

- **Given**: A claude subprocess writes non-JSON to stdout
- **When**: The dispatcher attempts to parse the stream-json output
- **Then**: The dispatcher logs a parse error
- **And**: Emits a `ResponseError` to the surface
- **And**: The daemon continues accepting requests

### Requirement: Structured Logging To Journal

The daemon MUST log via the `tracing` crate with `tracing-journald` as the subscriber. Log entries MUST include structured fields for:

- `session_id` — when the log is associated with a specific session
- `surface` — when the log is associated with a specific surface
- `turn_duration_ms` — on turn completion

The daemon MUST NOT create its own log files, write to stdout/stderr for logging purposes (systemd captures journal output), or implement log rotation.

#### Scenario: Turn completion is logged

- **Given**: The daemon processes a turn for session `abc-123`
- **When**: The turn completes
- **Then**: A journal entry is written with `session_id=abc-123`, `surface=dbus`, and `turn_duration_ms=<duration>`

### Requirement: One Daemon Per User

The daemon runs as a `systemd --user` service, which enforces exactly one instance per user session. The D-Bus well-known name `org.axios.Companion` provides an additional single-instance guarantee — if the name is already taken, the new instance fails to start.

The daemon MUST NOT implement its own lock file or PID file mechanism. Systemd and D-Bus name ownership are sufficient.

#### Scenario: Second instance attempt

- **Given**: One `companion-core` is already running for the user
- **When**: A second instance is launched manually
- **Then**: The second instance fails to acquire the D-Bus name and exits
- **And**: The first instance is unaffected

### Requirement: SIGHUP Is Reserved But Unimplemented

The daemon MUST NOT crash on receiving SIGHUP. It MUST log "SIGHUP received, no reload action defined" at info level and continue running. SIGHUP reload behavior is reserved for future use but is not specified in this version.

Persona files are immutable Nix store paths baked into the `companion` wrapper at build time. Changing persona requires `home-manager switch`, which restarts the service. Runtime reload adds no value at this tier.

#### Scenario: SIGHUP is sent

- **Given**: The daemon is running
- **When**: `kill -HUP <pid>` is sent
- **Then**: The daemon logs the event
- **And**: Continues running normally
