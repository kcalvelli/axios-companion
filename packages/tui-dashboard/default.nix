# companion-tui — terminal dashboard for the axios-companion daemon.
#
# Provides `companion-tui` binary that monitors the daemon via D-Bus.
# Enabled via services.axios-companion.tui.enable = true.
{
  lib,
  rustPlatform,
  pkg-config,
  dbus,
}:
rustPlatform.buildRustPackage {
  pname = "companion-tui";
  version = "0.1.0";

  src = ./.;

  cargoLock.lockFile = ./Cargo.lock;

  nativeBuildInputs = [ pkg-config ];
  buildInputs = [ dbus ];

  meta = {
    description = "axios-companion TUI dashboard — terminal-native monitoring for the companion daemon";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
}
