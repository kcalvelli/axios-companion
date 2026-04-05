# Tasks: Bootstrap — Tier 0 Shell Wrapper and Home-Manager Module

## Phase 1: Repository scaffolding

- [x] **1.1** Create repository skeleton (`flake.nix`, `README.md`, `LICENSE`, `.gitignore`, `ROADMAP.md`)
- [x] **1.2** Create `openspec/config.yaml` with context, non-goals, and architectural rules
- [x] **1.3** Create `openspec/changes/bootstrap/` with proposal, specs, and tasks (this document)
- [x] **1.4** Create skeleton proposals for all downstream tiers in `openspec/changes/`
- [x] **1.5** Initial commit and push to `github.com/kcalvelli/axios-companion`

## Phase 2: Nix package for the companion wrapper

- [ ] **2.1** Create `packages/companion/default.nix` — a `pkgs.writeShellApplication` that:
  - Accepts `claudePackage`, `personaBasePackage`, and `defaultWorkspace` as arguments
  - Builds a `companion` shell script with logic per `specs/wrapper/spec.md`
  - Includes runtime dependencies: `coreutils`, `jq` (for mcp config detection)
- [ ] **2.2** Write the wrapper shell script logic:
  - [ ] Parse arguments (separate companion-specific from passthrough)
  - [ ] Resolve persona files in documented order (base AGENT + base USER or user file + extras)
  - [ ] Concatenate persona files into a single system prompt string
  - [ ] Ensure workspace directory exists, run first-run scaffolding if empty
  - [ ] Auto-detect mcp-gateway config at documented paths
  - [ ] Build the final `claude` invocation with all flags
  - [ ] `exec` to `claude` so the exit code propagates transparently
- [ ] **2.3** Wire `packages.<system>.default` in `flake.nix` to build the companion package
- [ ] **2.4** Wire `overlays.default` in `flake.nix` to expose `axios-companion` as an overlay attr
- [ ] **2.5** Verify `nix build .#default` produces a working binary

## Phase 3: Default persona files

- [ ] **3.1** Create `persona/default/AGENT.md` per `specs/persona/spec.md`:
  - Response format rules only
  - Under 50 lines
  - Zero character voice, tone adjectives, or nostalgia framing
  - Explicit instruction to read `USER.md` for user context
- [ ] **3.2** Create `persona/default/USER.md` template per `specs/persona/spec.md`:
  - Header comment explaining purpose and how to customize
  - Placeholder sections: Who I am, Machines, Communication preferences, Things to check, Projects
  - All values are obvious placeholders (`<your name>`, etc.)
- [ ] **3.3** Create `packages/persona-default/default.nix` — a package that installs both files into a derivation referenced by `persona.basePackage`
- [ ] **3.4** Ensure the default persona package path is importable at Nix eval time so the module can reference individual files

## Phase 4: Home-manager module

- [ ] **4.1** Create `modules/home-manager/default.nix` with the option schema from `specs/home-manager/spec.md`:
  - [ ] `enable` — boolean
  - [ ] `package` — package (default: self)
  - [ ] `claudePackage` — package (default: `pkgs.claude-code`)
  - [ ] `persona.basePackage` — package (default: this flake's persona-default)
  - [ ] `persona.userFile` — nullable path
  - [ ] `persona.extraFiles` — list of paths
  - [ ] `workspaceDir` — string (default: `"${config.xdg.dataHome}/axios-companion/workspace"`)
  - [ ] `mcpConfigFile` — nullable path
- [ ] **4.2** Implement the `config` block:
  - [ ] Guard everything behind `lib.mkIf cfg.enable`
  - [ ] Build the wrapper package with options wired in (persona files resolved from package + module options)
  - [ ] Add the wrapper to `home.packages`
- [ ] **4.3** Wire `homeManagerModules.default` in `flake.nix` to point at `./modules/home-manager`
- [ ] **4.4** Verify module evaluates cleanly with `nix eval` or a test home-manager config

## Phase 5: Manual end-to-end testing

- [ ] **5.1** Test minimal enable: fresh home-manager config with only `services.axios-companion.enable = true`
  - `home-manager switch` succeeds
  - `which companion` finds the binary
  - `companion "hello"` runs and produces a response
  - First invocation creates the workspace directory with `README.md` and default `USER.md`
- [ ] **5.2** Test persona override: set `persona.userFile` to a custom file
  - Custom content replaces the default template in the system prompt
  - First invocation does NOT copy the default template into the workspace
- [ ] **5.3** Test extra persona files: layer character voice
  - Voice file is appended after user file
  - Companion adopts the voice described
- [ ] **5.4** Test mcp-gateway auto-detection: with mcp-gateway running on the same machine
  - Companion picks up the config from the auto-detect paths
  - Companion can invoke MCP tools from gateway servers
- [ ] **5.5** Test mcp-gateway absent: on a machine without mcp-gateway
  - Companion runs without warning or error
  - No `--mcp-config` flag is passed to `claude`
- [ ] **5.6** Test flag passthrough: `companion --resume`, `companion --model <name>`, `companion -p "prompt"`
  - All flags reach `claude` correctly
  - Companion's own flags still apply
- [ ] **5.7** Test exit code propagation
  - A successful `companion` call exits 0
  - A failing call exits with the same non-zero code as `claude`

## Phase 6: Documentation

- [ ] **6.1** Update `README.md` "Getting started" section with real working examples (remove the "Not yet functional" note)
- [ ] **6.2** Add a "First run" section explaining what the workspace is, what gets scaffolded, and how to customize
- [ ] **6.3** Add examples of persona override patterns: user context only, user context + voice, full custom persona
- [ ] **6.4** Document the `mcpConfigFile` auto-detect paths explicitly in README
- [ ] **6.5** Update `ROADMAP.md` to mark `bootstrap` as complete and point at the next proposal to tackle

## Phase 7: Validation and close

- [ ] **7.1** Run `nix flake check` — must pass
- [ ] **7.2** Verify a NixOS user who is not on axios can consume the flake and use the module (check: module code has zero axios references, all axios mentions are in docs only)
- [ ] **7.3** Verify multi-user scenario: two users on the same machine each enable the module independently and get isolated workspaces and configs
- [ ] **7.4** Archive this change to `openspec/changes/archive/bootstrap/` once all tasks above are checked
