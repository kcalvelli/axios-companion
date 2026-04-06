# companion-core — the axios-companion daemon.
#
# Persistent session manager and D-Bus control plane. Invokes the
# Tier 0 `companion` wrapper per turn; adds session mapping, surface
# multiplexing, and a systemd-integrated lifecycle.
{
  lib,
  rustPlatform,
}:
rustPlatform.buildRustPackage {
  pname = "companion-core";
  version = "0.1.0";

  src = ./.;

  cargoLock.lockFile = ./Cargo.lock;

  meta = {
    description = "axios-companion daemon — persistent session manager and D-Bus control plane";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
}
