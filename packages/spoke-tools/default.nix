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
  grim,
  wl-clipboard,
  systemd,
  xdg-utils,
  dex,
  niri,
}:
rustPlatform.buildRustPackage {
  pname = "companion-spoke-tools";
  version = "0.1.0";

  src = ./.;

  cargoLock.lockFile = ./Cargo.lock;

  nativeBuildInputs = [ makeWrapper ];

  # Per-tool shell-outs:
  #   libnotify    → notify-send             (notify)
  #   grim         → screen capture          (screenshot)
  #   wl-clipboard → wl-copy / wl-paste      (clipboard)
  #   systemd      → journalctl / systemctl  (journal)
  #   xdg-utils    → xdg-open                (apps: open_url)
  #   dex          → desktop-entry launcher  (apps: launch_desktop_entry)
  #   niri         → compositor IPC          (niri: all tools)
  # The `shell` tool has no bundled runtime deps — it spawns whatever
  # the operator's allowlist permits, directly from the process's
  # inherited PATH. That's the correct behavior for an allowlisted
  # command runner: the allowlist is the gate, PATH is just how the
  # binary gets found.
  # Each tool's runtime PATH is wrapped below so the shell-out resolves
  # regardless of the mcp-gateway unit's inherited PATH.
  buildInputs = [
    libnotify
    grim
    wl-clipboard
    systemd
    xdg-utils
    dex
    niri
  ];

  postInstall = ''
    wrapProgram $out/bin/companion-mcp-notify \
      --prefix PATH : ${lib.makeBinPath [ libnotify ]}

    wrapProgram $out/bin/companion-mcp-screenshot \
      --prefix PATH : ${lib.makeBinPath [ grim ]}

    wrapProgram $out/bin/companion-mcp-clipboard \
      --prefix PATH : ${lib.makeBinPath [ wl-clipboard ]}

    wrapProgram $out/bin/companion-mcp-journal \
      --prefix PATH : ${lib.makeBinPath [ systemd ]}

    wrapProgram $out/bin/companion-mcp-apps \
      --prefix PATH : ${lib.makeBinPath [ xdg-utils dex ]}

    wrapProgram $out/bin/companion-mcp-niri \
      --prefix PATH : ${lib.makeBinPath [ niri ]}
  '';

  meta = {
    description = "MCP tool servers exposing local-machine capabilities for cairn-companion";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
}
