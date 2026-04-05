# Companion Wrapper Specification

## Purpose

The `companion` binary is the user-facing entry point for axios-companion Tier 0. It is a shell wrapper that invokes the `claude` CLI with persona files, workspace directory, and (if present) mcp-gateway configuration pre-loaded — turning a stateless `claude` invocation into a persona-aware companion without any persistent process.

## ADDED Requirements

### Requirement: Binary Is A Pure Shell Wrapper

The `companion` binary MUST be implemented as a `pkgs.writeShellApplication`, not a compiled binary in a compiled language, for Tier 0. This keeps the implementation transparent, easily reviewable, and trivially debuggable.

#### Scenario: User inspects the binary

- **Given**: A user has `services.axios-companion.enable = true`
- **When**: The user runs `cat $(which companion)`
- **Then**: The output is a shell script they can read
- **And**: The script is less than 200 lines
- **And**: The script's logic is obvious from reading it

### Requirement: Persona Resolution Order

The wrapper MUST assemble the system prompt from persona files in the following order, concatenated with blank-line separators:

1. Default base `AGENT.md` from `persona.basePackage`
2. User-provided `USER.md` from `persona.userFile` if set, OR the default `USER.md` template from `persona.basePackage` if not set
3. Each file in `persona.extraFiles` in the order they appear in the list

Later files in the order MAY add to, override, or contradict earlier files. The wrapper does not interpret file content — it concatenates.

#### Scenario: User has only default persona

- **Given**: `services.axios-companion.persona.userFile = null` and `persona.extraFiles = [ ]`
- **When**: The user runs `companion "hello"`
- **Then**: The system prompt passed to Claude is the concatenation of default `AGENT.md` and default `USER.md` template
- **And**: The total system prompt includes both files' contents in that order

#### Scenario: User overrides USER.md

- **Given**: `services.axios-companion.persona.userFile = ./my-context.md`
- **When**: The user runs `companion "hello"`
- **Then**: The system prompt contains default `AGENT.md` followed by the contents of `./my-context.md`
- **And**: The default `USER.md` template is NOT included

#### Scenario: User adds character persona files

- **Given**: `persona.userFile = ./my-context.md` and `persona.extraFiles = [ ./voice.md ./preferences.md ]`
- **When**: The user runs `companion "hello"`
- **Then**: The system prompt is: default `AGENT.md` + `./my-context.md` + `./voice.md` + `./preferences.md`

### Requirement: Workspace Directory Creation On First Run

The wrapper MUST ensure the workspace directory exists before invoking Claude. If the directory does not exist, the wrapper creates it along with a `README.md` explaining its purpose, and copies the default `USER.md` template into it if the user has not provided a user file via module options.

#### Scenario: First-ever invocation on a fresh system

- **Given**: `services.axios-companion.workspaceDir = "$XDG_DATA_HOME/axios-companion/workspace"` (default)
- **And**: The directory does not exist
- **When**: The user runs `companion "hello"` for the first time
- **Then**: The directory is created
- **And**: A `README.md` is written explaining what the workspace is for
- **And**: A `USER.md` copy of the default template is written (only if `persona.userFile` is null)
- **And**: Claude is then invoked as normal

#### Scenario: Workspace already exists

- **Given**: The workspace directory already exists with user files in it
- **When**: The user runs `companion "hello"`
- **Then**: The wrapper does NOT modify or overwrite any existing files in the workspace
- **And**: Claude is invoked normally

### Requirement: MCP Config Auto-Detection

The wrapper MUST check the following paths in order when `mcpConfigFile` is null (the default), and use the first one that exists:

1. `$XDG_CONFIG_HOME/mcp-gateway/claude_config.json`
2. `$XDG_CONFIG_HOME/mcp/mcp_servers.json`
3. `$HOME/.mcp.json`

If none exist, the wrapper MUST invoke Claude without `--mcp-config`. The wrapper MUST NOT error or warn if no MCP config is found — MCP tools are optional.

#### Scenario: User has mcp-gateway running

- **Given**: mcp-gateway has generated `$XDG_CONFIG_HOME/mcp-gateway/claude_config.json`
- **When**: The user runs `companion "list my github notifications"`
- **Then**: The wrapper detects the file and passes `--mcp-config <path>` to Claude
- **And**: Claude has access to every MCP server mcp-gateway exposes

#### Scenario: User explicitly sets mcpConfigFile

- **Given**: `services.axios-companion.mcpConfigFile = "/custom/path.json"`
- **When**: The user runs `companion "hello"`
- **Then**: The wrapper uses `/custom/path.json` and does NOT auto-detect
- **And**: If the file does not exist, the wrapper prints a warning to stderr but still invokes Claude

#### Scenario: No MCP config available

- **Given**: No MCP config files exist at any auto-detect path
- **And**: `mcpConfigFile` is null
- **When**: The user runs `companion "hello"`
- **Then**: The wrapper invokes Claude without `--mcp-config`
- **And**: The wrapper does not warn or error

### Requirement: Argument Passthrough

Arguments that the wrapper does not consume MUST be passed through to the underlying `claude` invocation verbatim. The wrapper MUST NOT intercept or rewrite Claude Code flags.

#### Scenario: User passes a one-shot prompt

- **Given**: The user runs `companion "what is the capital of Montana"`
- **When**: The wrapper invokes `claude`
- **Then**: The command is effectively `claude --append-system-prompt "<persona>" --add-dir "<workspace>" [--mcp-config <path>] -p "what is the capital of Montana"`

#### Scenario: User passes Claude Code flags

- **Given**: The user runs `companion --resume --model claude-haiku-4-5`
- **When**: The wrapper invokes `claude`
- **Then**: The `--resume` and `--model claude-haiku-4-5` flags are passed through to `claude`
- **And**: The wrapper's persona/workspace/mcp flags are ALSO applied

#### Scenario: User runs interactive mode

- **Given**: The user runs `companion` with no arguments
- **When**: The wrapper invokes `claude`
- **Then**: `claude` is started in interactive mode with persona/workspace/mcp pre-loaded

### Requirement: Exit Code Transparency

The wrapper MUST exit with the exit code of the underlying `claude` invocation. It MUST NOT swallow, translate, or wrap exit codes.

#### Scenario: Claude exits successfully

- **Given**: `claude -p "hello"` would exit 0
- **When**: `companion "hello"` runs
- **Then**: `companion` exits 0

#### Scenario: Claude errors

- **Given**: `claude -p "hello"` would exit with non-zero status
- **When**: `companion "hello"` runs
- **Then**: `companion` exits with the same non-zero status

### Requirement: No Persistent State Between Invocations

The wrapper MUST NOT write any state that persists between invocations beyond first-run workspace scaffolding. It MUST NOT maintain a lock file, cache, log, or session record of its own. (Claude Code's own `~/.claude/projects/` session storage is unaffected and is Claude Code's concern, not the wrapper's.)

#### Scenario: Two invocations in sequence

- **Given**: A user has the workspace already created
- **When**: The user runs `companion "first question"` and then `companion "second question"`
- **Then**: The wrapper creates no files, logs, or state between the two invocations
- **And**: Any conversation continuity is handled by Claude Code's own `--resume` flag
