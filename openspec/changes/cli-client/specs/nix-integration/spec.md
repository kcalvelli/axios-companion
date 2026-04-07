# Spec: Nix Integration — cli-client

## Summary

The CLI ships as a Rust package built by `rustPlatform.buildRustPackage`,
exposed via `flake.nix` as `packages.<system>.companion-cli`, and wired
into the home-manager module under `services.axios-companion.cli.*`.

## Package

- Crate name: `companion-cli`
- Binary name: `companion`
- Location: `packages/cli-client/`
- Build inputs: `pkg-config`, `dbus` (for zbus)

## Flake Output

```nix
packages.<system>.companion-cli = pkgs.callPackage ./packages/cli-client { };
```

## Home-Manager Options

### `services.axios-companion.cli.enable`

Type: `bool`, default: `false`

Assertion: `cli.enable → daemon.enable` (the CLI talks D-Bus; no daemon = no
one to talk to).

### `services.axios-companion.cli.package`

Type: `package`, default: `self.packages.<system>.companion-cli`

## PATH Resolution

When `cli.enable = false`:
- `home.packages` contains the shell wrapper (`companion` binary)
- Behavior unchanged from Tier 0

When `cli.enable = true`:
- `home.packages` contains:
  - `cfg.cli.package` → provides `companion` binary (the Rust CLI)
  - `companionRaw` → symlink derivation providing `companion-raw` → shell wrapper
- The shell wrapper is NOT on the user's PATH as `companion`
- The daemon's systemd service PATH still includes the wrapper package
  (`${cfg.package}/bin`), so the daemon's dispatcher finds `companion`
  there independently

## Backward Compatibility

The daemon's dispatcher calls `companion` (the shell wrapper) via its
service-scoped PATH. This is unaffected by CLI activation — the daemon
never looks at the user's shell PATH.

Users who need the raw wrapper while the CLI is active can invoke
`companion-raw`.
