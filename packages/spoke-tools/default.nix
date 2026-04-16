# companion-spoke-tools — machine-local MCP tool servers.
#
# One cargo package, multiple [[bin]] entries. Each binary is a
# short-lived stdio MCP server spawned per-call by mcp-gateway. This
# package builds all of them in one shot; the home-manager module
# picks which ones to register with mcp-gateway via the
# `services.cairn-companion.spoke.tools.<tool>.enable` toggles.
#
# Each tool's runtime dependencies (notify-send, grim, wl-clipboard,
# etc.) are added to the wrapped binary's PATH via makeWrapper so the
# tool works regardless of what PATH mcp-gateway's systemd unit
# happens to have.
{
  lib,
  rustPlatform,
  makeWrapper,
  libnotify,
}:
rustPlatform.buildRustPackage {
  pname = "companion-spoke-tools";
  version = "0.1.0";

  src = ./.;

  cargoLock.lockFile = ./Cargo.lock;

  nativeBuildInputs = [ makeWrapper ];

  # The libnotify package provides notify-send. Listed here as a build
  # input so it ends up in the package's runtime closure; makeWrapper
  # below prepends its bin dir to PATH.
  buildInputs = [ libnotify ];

  # Wrap each binary with its per-tool runtime PATH. Currently only
  # notify; future tools (screenshot, clipboard, niri, etc.) add their
  # own wrap step here.
  postInstall = ''
    wrapProgram $out/bin/companion-mcp-notify \
      --prefix PATH : ${lib.makeBinPath [ libnotify ]}
  '';

  meta = {
    description = "MCP tool servers exposing local-machine capabilities for cairn-companion";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
}
