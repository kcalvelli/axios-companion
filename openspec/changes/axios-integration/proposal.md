# Proposal: Axios Integration — Consumer-Side Wiring

> **Status**: Skeleton — this proposal is a roadmap placeholder. Unlike other proposals in this repo, the actual change artifact for this one will live in the [axios](https://github.com/kcalvelli/axios) repository under `axios/openspec/changes/`, not here. This placeholder exists so the roadmap is complete from axios-companion's perspective.

## Tier

Consumer-side integration (not a tier of axios-companion itself)

## Summary

Add `axios-companion` as a flake input to axios, import the home-manager module into axios's home profile, and expose companion-related options under axios's own module namespace so axios users get sensible defaults for their environment (Niri-aware spoke tools, DMS-aware notifications, agenix-backed secret file paths, default MCP gateway integration).

## Motivation

axios-companion is designed to work on any NixOS system, but axios users should get the best out-of-the-box experience because axios is the canonical environment this project is designed for. That means: the companion should be enableable with a single option in an axios user's home config, with defaults that match axios's conventions (agenix secret paths, mcp-gateway integration, Niri/DMS-aware tool selection).

This is a thin integration layer. It does not modify the axios-companion repository — it only adds consumption glue to axios. Publishing this as a separate proposal keeps axios's SDD history honest about what changed on the axios side when axios-companion landed.

## Scope

### In scope (lives in axios repo)

- Add `axios-companion` as a flake input in `axios/flake.nix`
- Create `axios/home/ai/companion.nix` that imports `inputs.axios-companion.homeManagerModules.default`
- Add `services.axios.companion.enable` option (axios-side wrapper) that:
  - Enables `services.axios-companion.enable` with defaults suitable for axios users
  - Wires agenix secret file paths for channel credentials (telegram, email, discord, xmpp)
  - Points `mcpConfigFile` at axios's known mcp-gateway output location
  - When Niri + DMS is detected, enables the Niri-aware spoke tools automatically
- Update axios's `home/ai/default.nix` to include the companion module
- Add documentation in axios README about enabling the companion

### Out of scope

- Modifying the `axios-companion` repository — this is purely an axios-side change
- Forking or patching `axios-companion` — all changes go upstream via PRs to `axios-companion`
- Custom axios-specific tool servers — if a tool is useful for axios users, it should ship in `axios-companion` and be opt-in via module options, not axios-specific

### Non-goals

- Making `axios-companion` depend on axios (the whole point of the separate-repo design is the opposite)
- Replacing axios users' manual configuration — axios users can still override any option

## Dependencies

- `bootstrap` must be shipped (the module must exist to be imported)
- Ideally also `daemon-core` and `cli-client` so axios users get the full CLI experience on first enable — but the axios integration can land with just Tier 0 and add Tier 1 later

## Success criteria

1. An axios user can add `services.axios.companion.enable = true;` to their home configuration, run `home-manager switch`, and get a working companion with sensible axios defaults
2. axios's agenix secrets (telegram bot token, email password, etc.) are automatically wired when the corresponding channels are enabled
3. axios users on Niri + DMS automatically get Wayland-native spoke tools without configuring them individually
4. Non-axios users consuming `axios-companion` directly continue to work unchanged — this proposal adds nothing to the companion repo itself
5. The change is documented in both axios and axios-companion README files, with the axios side being the primary documentation
