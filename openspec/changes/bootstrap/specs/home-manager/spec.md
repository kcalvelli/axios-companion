# Home-Manager Module Specification

## Purpose

The home-manager module exposes `services.axios-companion` with declarative options for enabling and configuring the Tier 0 companion wrapper. It integrates with the user's home environment by installing the `companion` binary into their user package set and ensuring the workspace directory exists.

## ADDED Requirements

### Requirement: Module Provides `services.axios-companion` Namespace

The module MUST expose all options under `services.axios-companion.*` and MUST NOT pollute any other options namespace. The module MUST be importable via `imports = [ inputs.axios-companion.homeManagerModules.default ];` and MUST NOT require any additional setup beyond the import.

#### Scenario: User imports the module

- **Given**: A home-manager configuration with `imports = [ inputs.axios-companion.homeManagerModules.default ];`
- **When**: The user runs `home-manager switch`
- **Then**: The configuration evaluates successfully
- **And**: The option `services.axios-companion.enable` is available
- **And**: No axios-specific or non-companion options are introduced

### Requirement: Enable Option Gates All Behavior

When `services.axios-companion.enable = false` (or the option is unset), the module MUST produce no effect. It MUST NOT install the `companion` binary, create workspace directories, or write any activation scripts. Only when `enable = true` does the module become active.

#### Scenario: Module imported but not enabled

- **Given**: The module is imported but `services.axios-companion.enable` is not set
- **When**: `home-manager switch` runs
- **Then**: The `companion` binary is NOT added to `home.packages`
- **And**: No workspace directory is created
- **And**: No activation scripts run

### Requirement: Package Option Defaults To Flake's Own Package

The `package` option MUST default to `self.packages.${pkgs.system}.default` (the companion wrapper package built by this flake). Users MAY override it to use a custom build, e.g. a fork.

#### Scenario: User uses default package

- **Given**: `services.axios-companion.enable = true` with no other options set
- **When**: `home-manager switch` runs
- **Then**: The `companion` binary installed comes from `inputs.axios-companion.packages.${system}.default`

#### Scenario: User overrides the package

- **Given**: `services.axios-companion.package = myOverriddenCompanionPkg`
- **When**: `home-manager switch` runs
- **Then**: The `companion` binary installed comes from `myOverriddenCompanionPkg`

### Requirement: Claude Code Package Is Configurable

The `claudePackage` option MUST default to `pkgs.claude-code` and MUST be passed into the wrapper's build so that the wrapper invokes the specific Claude Code binary configured. Users MAY override it to select a different version.

#### Scenario: Default claude-code package

- **Given**: `services.axios-companion.enable = true`
- **When**: The wrapper is invoked
- **Then**: The `claude` binary it calls is from `pkgs.claude-code`

#### Scenario: User pins a specific claude-code version

- **Given**: `services.axios-companion.claudePackage = inputs.claude-code-older.packages.${system}.default`
- **When**: The wrapper is invoked
- **Then**: The `claude` binary it calls is from the user-specified package

### Requirement: Persona Options Layer Content In A Documented Order

The module MUST provide `persona.basePackage`, `persona.userFile`, and `persona.extraFiles` options. The base package supplies default `AGENT.md` and `USER.md` template files. The user file, if set, replaces the default `USER.md` in the resolution order. Extra files are appended after in list order.

#### Scenario: Option defaults

- **Given**: `services.axios-companion.enable = true` with no persona options set
- **When**: The module evaluates
- **Then**: `persona.basePackage` resolves to this flake's default persona package
- **And**: `persona.userFile` is null
- **And**: `persona.extraFiles` is an empty list

#### Scenario: User provides their own context file

- **Given**: `services.axios-companion.persona.userFile = ./my-about-me.md`
- **When**: The wrapper invokes Claude
- **Then**: The wrapper uses `./my-about-me.md` instead of the default `USER.md` template

#### Scenario: User layers character persona files on top

- **Given**: `persona.userFile = ./me.md` and `persona.extraFiles = [ ./voice.md ./prefs.md ]`
- **When**: The wrapper invokes Claude
- **Then**: The assembled system prompt order is: default `AGENT.md` → `./me.md` → `./voice.md` → `./prefs.md`

### Requirement: Workspace Directory Option With Sensible Default

The `workspaceDir` option MUST default to `"${config.xdg.dataHome}/axios-companion/workspace"`. Users MAY override it to point at any absolute path they prefer (e.g. a synced directory, a git repo they maintain manually).

#### Scenario: Default workspace location

- **Given**: `services.axios-companion.enable = true` with `workspaceDir` unset
- **When**: The user runs `companion "hello"` for the first time
- **Then**: The workspace is created at `$XDG_DATA_HOME/axios-companion/workspace` (typically `~/.local/share/axios-companion/workspace`)

#### Scenario: User points workspace at a sync'd repo

- **Given**: `services.axios-companion.workspaceDir = "/home/keith/sync/my-companion-workspace"`
- **When**: The user runs `companion "hello"`
- **Then**: The wrapper uses the user-specified path
- **And**: First-run scaffolding only runs if that directory is empty or missing

### Requirement: MCP Config File Option With Auto-Detection Fallback

The `mcpConfigFile` option MUST default to `null`, which triggers the wrapper's auto-detection logic (see wrapper spec). When set to a path, the wrapper MUST use that path explicitly and MUST NOT auto-detect.

#### Scenario: Default (auto-detect)

- **Given**: `services.axios-companion.mcpConfigFile = null` (default)
- **When**: The wrapper runs
- **Then**: It checks the auto-detect paths in order and uses the first one that exists

#### Scenario: Explicit mcp config path

- **Given**: `services.axios-companion.mcpConfigFile = "/etc/custom-mcp.json"`
- **When**: The wrapper runs
- **Then**: It uses `/etc/custom-mcp.json` without auto-detection

### Requirement: Module Adds Companion To `home.packages`

When enabled, the module MUST add the resolved `package` (with `claudePackage` and persona files wired in) to `home.packages` so that the `companion` binary is available on the user's PATH after `home-manager switch`.

#### Scenario: Binary is on PATH after switch

- **Given**: The module is enabled
- **When**: The user runs `home-manager switch` and then opens a fresh shell
- **Then**: `which companion` returns a valid path
- **And**: `companion --help` or `companion "hello"` can be executed from any directory
