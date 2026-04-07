# companion-cli — the Rust CLI for axios-companion.
#
# Provides `companion` binary that talks to the daemon via D-Bus.
# Replaces the Tier 0 shell wrapper on the user's PATH when
# services.axios-companion.cli.enable = true.
{
  lib,
  rustPlatform,
  pkg-config,
  dbus,
}:
rustPlatform.buildRustPackage {
  pname = "companion-cli";
  version = "0.1.0";

  src = ./.;

  cargoLock.lockFile = ./Cargo.lock;

  nativeBuildInputs = [ pkg-config ];
  buildInputs = [ dbus ];

  meta = {
    description = "axios-companion CLI — command-line interface for the companion daemon";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
}
