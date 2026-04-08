# axios-companion home-manager module.
#
# Tier 0: Exposes `services.axios-companion.*` options and, when enabled,
# installs a per-user `companion` wrapper binary that invokes `claude`
# with persona files, workspace directory, and any mcp-gateway config.
#
# Tier 1: Adds `services.axios-companion.daemon.*` options. When
# `daemon.enable = true`, installs and starts the `companion-core`
# systemd user service (D-Bus control plane, session routing, etc.).
#
# This module is a thin closure over the flake's `self` so it can reach
# `self.lib.<system>.buildCompanion` — the public helper that builds the
# wrapper with the caller's resolved options. See
# openspec/changes/bootstrap/specs/home-manager/spec.md for the contract.
{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.axios-companion;

  # Per-user build of the wrapper, using the flake-exposed helper rather
  # than consuming `self.packages.${pkgs.system}.default` (the reference
  # build). This is what the home-manager spec requires.
  builtCompanion = self.lib.${pkgs.system}.buildCompanion {
    claudePackage = cfg.claudePackage;
    personaBasePackage = cfg.persona.basePackage;
    userFile = cfg.persona.userFile;
    extraFiles = cfg.persona.extraFiles;
    defaultWorkspace = cfg.workspaceDir;
    mcpConfigFile = cfg.mcpConfigFile;
  };

  # When the CLI is active, the shell wrapper's `companion` binary would
  # collide with the CLI's `companion` binary. This alias package exposes
  # the wrapper as `companion-code` so users can bypass the daemon.
  companionRaw = pkgs.runCommand "companion-code" { } ''
    mkdir -p $out/bin
    ln -s ${builtCompanion}/bin/companion $out/bin/companion-code
  '';
in
{
  options.services.axios-companion = {
    enable = lib.mkEnableOption "axios-companion Tier 0 wrapper around Claude Code";

    package = lib.mkOption {
      type = lib.types.package;
      default = builtCompanion;
      defaultText = lib.literalMD ''
        a per-user build produced by `lib.buildCompanion` using the other
        `services.axios-companion.*` options
      '';
      description = ''
        The resolved companion wrapper package to install. Defaults to a
        per-user build assembled from the other options in this module.
        Override only if you have a reason to substitute your own build
        (e.g. a local fork).
      '';
    };

    claudePackage = lib.mkOption {
      type = lib.types.package;
      default = pkgs.claude-code;
      defaultText = lib.literalExpression "pkgs.claude-code";
      description = ''
        The Claude Code CLI package the wrapper invokes. Note that
        `pkgs.claude-code` is marked unfree in nixpkgs; consumers must
        permit it via `nixpkgs.config.allowUnfree = true` or a narrow
        `allowUnfreePredicate` in their home-manager configuration.
      '';
    };

    persona = {
      basePackage = lib.mkOption {
        type = lib.types.package;
        default = self.packages.${pkgs.system}.personaDefault;
        defaultText = lib.literalExpression "inputs.axios-companion.packages.\${pkgs.system}.personaDefault";
        description = ''
          Package containing the default `AGENT.md` and `USER.md` files.
          The wrapper reads both files from this package at build time
          and bakes their store paths into the generated script. Override
          only if you want to replace the character-free defaults with a
          different base package.
        '';
      };

      userFile = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        example = lib.literalExpression "./my-context.md";
        description = ''
          Optional path to a user-written context file. When set, this
          file replaces the default `USER.md` template in the persona
          resolution order, and the first-run workspace scaffolder does
          NOT copy the default template into the workspace.
        '';
      };

      extraFiles = lib.mkOption {
        type = lib.types.listOf lib.types.path;
        default = [ ];
        example = lib.literalExpression "[ ./voice.md ./preferences.md ]";
        description = ''
          Additional persona files appended after the user file (or the
          default `USER.md` template if `userFile` is null). Files are
          concatenated in list order with blank-line separators; later
          files may extend or override earlier ones.
        '';
      };
    };

    workspaceDir = lib.mkOption {
      type = lib.types.str;
      default = "${config.xdg.dataHome}/axios-companion/workspace";
      defaultText = lib.literalExpression "\"\${config.xdg.dataHome}/axios-companion/workspace\"";
      description = ''
        Absolute path to the companion workspace directory. The wrapper
        ensures this directory exists on first run (creating `README.md`
        and, unless `persona.userFile` is set, a copy of the default
        `USER.md` template). Change this to point at a synced location
        (git repo, syncthing share, Tailscale Drive path) if you want
        the workspace to follow you across machines.
      '';
    };

    mcpConfigFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      example = lib.literalExpression "\"\${config.xdg.configHome}/mcp-gateway/claude_config.json\"";
      description = ''
        Explicit path to an mcp-gateway config file. When null (the
        default), the wrapper auto-detects common locations at runtime:
        `$XDG_CONFIG_HOME/mcp-gateway/claude_config.json`,
        `$XDG_CONFIG_HOME/mcp/mcp_servers.json`, then `$HOME/.mcp.json`.
        When set, the wrapper uses this path exclusively.
      '';
    };

    daemon = {
      enable = lib.mkEnableOption "companion-core daemon (Tier 1 D-Bus service)";

      package = lib.mkOption {
        type = lib.types.package;
        default = self.packages.${pkgs.system}.companion-core;
        defaultText = lib.literalExpression "inputs.axios-companion.packages.\${pkgs.system}.companion-core";
        description = ''
          The companion-core daemon package. Override only if you have a
          local fork or want to pin a specific build.
        '';
      };
    };

    cli = {
      enable = lib.mkEnableOption "companion CLI (Tier 1 Rust binary replacing the shell wrapper)";

      package = lib.mkOption {
        type = lib.types.package;
        default = self.packages.${pkgs.system}.companion-cli;
        defaultText = lib.literalExpression "inputs.axios-companion.packages.\${pkgs.system}.companion-cli";
        description = ''
          The companion CLI package. When enabled, this replaces the Tier 0
          shell wrapper as the `companion` binary on the user's PATH. The
          shell wrapper remains available as `companion-code`.
        '';
      };
    };

    tui = {
      enable = lib.mkEnableOption "companion TUI dashboard (Tier 1 terminal monitoring)";

      package = lib.mkOption {
        type = lib.types.package;
        default = self.packages.${pkgs.system}.companion-tui;
        defaultText = lib.literalExpression "inputs.axios-companion.packages.\${pkgs.system}.companion-tui";
        description = ''
          The companion TUI dashboard package. Provides `companion-tui`
          binary for terminal-native daemon monitoring.
        '';
      };
    };

    channels.telegram = {
      enable = lib.mkEnableOption "Telegram channel adapter inside the companion daemon";

      botTokenFile = lib.mkOption {
        type = lib.types.path;
        description = ''
          Path to a file containing the Telegram bot token (one line,
          no trailing newline). Compatible with agenix-managed secrets.
        '';
        example = lib.literalExpression "/run/agenix/telegram-bot-token";
      };

      allowedUsers = lib.mkOption {
        type = lib.types.listOf lib.types.int;
        default = [ ];
        description = ''
          List of Telegram user IDs allowed to message the bot.
          Empty list means nobody gets through — deny by default.
          Find your user ID by messaging @userinfobot on Telegram.
        '';
        example = [ 123456789 ];
      };

      mentionOnly = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = ''
          When true, the bot only responds in group chats when
          @mentioned. Private messages are always handled regardless
          of this setting.
        '';
      };

      streamMode = lib.mkOption {
        type = lib.types.enum [ "single_message" "multi_message" ];
        default = "single_message";
        description = ''
          How to render streaming responses.
          `single_message`: edit a single message in place as chunks arrive.
          `multi_message`: collect full response, split at 4096-char boundaries.
        '';
      };
    };

    channels.xmpp = {
      enable = lib.mkEnableOption "XMPP channel adapter inside the companion daemon";

      jid = lib.mkOption {
        type = lib.types.str;
        description = ''
          Bare JID for the bot (e.g. `sid@chat.example.org`). The account
          must already exist on the XMPP server — the daemon does not
          register accounts.
        '';
        example = "sid@chat.taile0fb4.ts.net";
      };

      passwordFile = lib.mkOption {
        type = lib.types.path;
        description = ''
          Path to a file containing the bot's XMPP password (one line,
          no trailing newline). Compatible with agenix-managed secrets.
        '';
        example = lib.literalExpression "/run/agenix/xmpp-bot-password";
      };

      server = lib.mkOption {
        type = lib.types.str;
        default = "127.0.0.1";
        description = ''
          Hostname or IP to connect to. Defaults to loopback because the
          most common deployment runs the bot on the same host as Prosody.
          Set to a Tailscale hostname (e.g. `chat.taile0fb4.ts.net`) to
          connect through a Tailscale Serve TCP-passthrough endpoint.
          DNS SRV records are NOT used — the daemon connects directly to
          the address you give it.
        '';
      };

      port = lib.mkOption {
        type = lib.types.port;
        default = 5222;
        description = ''
          TCP port for client-to-server XMPP. Default 5222 covers both
          loopback Prosody and Tailscale Serve `--tcp=5222` passthrough.
        '';
      };

      allowedJids = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        description = ''
          Bare JIDs allowed to DM the bot. Empty list means nobody gets
          through — deny by default, matching the telegram channel.
        '';
        example = [ "keith@chat.example.org" ];
      };

      mucRooms = lib.mkOption {
        type = lib.types.listOf (lib.types.submodule {
          options = {
            jid = lib.mkOption {
              type = lib.types.str;
              description = "Bare JID of the MUC room.";
              example = "xojabo@muc.chat.example.org";
            };
            nick = lib.mkOption {
              type = lib.types.str;
              description = "Nick to use when joining the room.";
              example = "Sid";
            };
          };
        });
        default = [ ];
        description = ''
          MUC rooms to auto-join on connection. NOTE: Phase 5 of the
          channel-xmpp openspec change has not yet shipped, so configuring
          rooms here causes the env var to be set but the daemon will log
          "MUC join deferred to Phase 5" and not actually join. When Phase
          5 lands, the join activates automatically on next rebuild.
        '';
        example = lib.literalExpression ''
          [ { jid = "xojabo@muc.chat.example.org"; nick = "Sid"; } ]
        '';
      };

      mentionOnly = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          When true, the bot only responds in MUC rooms when explicitly
          addressed by nick. **Inverted from telegram's default** (false)
          because high-volume household rooms like xojabo would otherwise
          burn tokens on every message.
        '';
      };

      streamMode = lib.mkOption {
        type = lib.types.enum [ "single_message" "multi_message" ];
        default = "single_message";
        description = ''
          How to render streaming responses.
          `single_message`: send the first chunk, then update via XEP-0308
          Last Message Correction stanzas as chunks arrive.
          `multi_message`: collect full response, split at ~3000-char
          boundaries (the empirical comfortable size for Conversations,
          Gajim, and Dino).
          NOTE: Phase 4 of channel-xmpp has not yet shipped — the current
          handler always collects to a single final stanza regardless of
          this setting. The mode is wired so the env var works once the
          streaming code lands.
        '';
      };
    };

    gateway.openai = {
      enable = lib.mkEnableOption "OpenAI-compatible HTTP gateway inside the companion daemon";

      port = lib.mkOption {
        type = lib.types.port;
        default = 18789;
        description = ''
          TCP port for the OpenAI gateway. Default matches ZeroClaw's
          port for migration parity.
        '';
      };

      bindAddress = lib.mkOption {
        type = lib.types.str;
        default = "0.0.0.0";
        description = ''
          Bind address for the gateway listener. Default binds all
          interfaces; Tailscale ACLs are the access control boundary.
        '';
      };

      modelName = lib.mkOption {
        type = lib.types.str;
        default = "companion";
        description = ''
          Model name returned by `/v1/models` and echoed in completion
          responses. Cosmetic — the companion always routes through the
          same claude backend regardless of what this says.
        '';
      };

      sessionPolicy = lib.mkOption {
        type = lib.types.enum [ "per-conversation-id" "single-session" "ephemeral" ];
        default = "per-conversation-id";
        description = ''
          How HTTP requests map to dispatcher sessions.
          `per-conversation-id`: honor `X-Conversation-ID` header, default
          to a shared session. `single-session`: all gateway traffic shares
          one session. `ephemeral`: every request gets a fresh session.
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      assertions = [
        {
          assertion = cfg.cli.enable -> cfg.daemon.enable;
          message = "services.axios-companion.cli requires daemon.enable — the CLI talks to the daemon via D-Bus";
        }
        {
          assertion = cfg.tui.enable -> cfg.daemon.enable;
          message = "services.axios-companion.tui requires daemon.enable — the TUI talks to the daemon via D-Bus";
        }
        {
          assertion = cfg.channels.telegram.enable -> cfg.daemon.enable;
          message = "services.axios-companion.channels.telegram requires daemon.enable — the adapter runs inside the daemon";
        }
        {
          assertion = cfg.channels.xmpp.enable -> cfg.daemon.enable;
          message = "services.axios-companion.channels.xmpp requires daemon.enable — the adapter runs inside the daemon";
        }
      ];

      # When the CLI is active it owns the `companion` name on the user's
      # PATH. The shell wrapper is still installed as `companion-code`.
      # When the CLI is off, the shell wrapper is installed as `companion`.
      home.packages =
        if cfg.cli.enable then
          [ cfg.cli.package companionRaw ]
        else
          [ cfg.package ];
    }

    (lib.mkIf cfg.daemon.enable {
      # Install the daemon binary.
      home.packages = [ cfg.daemon.package ];

      # Systemd user service for the daemon.
      systemd.user.services.companion-core = {
        Unit = {
          Description = "axios-companion daemon";
          Documentation = "https://github.com/kcalvelli/axios-companion";
        };

        Service = {
          Type = "notify";
          ExecStart = "${cfg.daemon.package}/bin/companion-core";
          Restart = "on-failure";
          RestartSec = 5;
          TimeoutStopSec = 130;
          Environment = [
            "XDG_DATA_HOME=${config.xdg.dataHome}"
            "PATH=${cfg.package}/bin:${cfg.daemon.package}/bin:/run/current-system/sw/bin"
          ] ++ lib.optionals cfg.gateway.openai.enable [
            "COMPANION_GATEWAY_ENABLE=1"
            "COMPANION_GATEWAY_PORT=${toString cfg.gateway.openai.port}"
            "COMPANION_GATEWAY_BIND=${cfg.gateway.openai.bindAddress}"
            "COMPANION_GATEWAY_MODEL=${cfg.gateway.openai.modelName}"
            "COMPANION_GATEWAY_SESSION_POLICY=${cfg.gateway.openai.sessionPolicy}"
          ] ++ lib.optionals cfg.channels.telegram.enable [
            "COMPANION_TELEGRAM_ENABLE=1"
            "COMPANION_TELEGRAM_BOT_TOKEN_FILE=${cfg.channels.telegram.botTokenFile}"
            "COMPANION_TELEGRAM_ALLOWED_USERS=${lib.concatMapStringsSep "," toString cfg.channels.telegram.allowedUsers}"
            "COMPANION_TELEGRAM_MENTION_ONLY=${if cfg.channels.telegram.mentionOnly then "1" else "0"}"
            "COMPANION_TELEGRAM_STREAM_MODE=${cfg.channels.telegram.streamMode}"
          ] ++ lib.optionals cfg.channels.xmpp.enable [
            "COMPANION_XMPP_ENABLE=1"
            "COMPANION_XMPP_JID=${cfg.channels.xmpp.jid}"
            "COMPANION_XMPP_PASSWORD_FILE=${cfg.channels.xmpp.passwordFile}"
            "COMPANION_XMPP_SERVER=${cfg.channels.xmpp.server}"
            "COMPANION_XMPP_PORT=${toString cfg.channels.xmpp.port}"
            "COMPANION_XMPP_ALLOWED_JIDS=${lib.concatStringsSep "," cfg.channels.xmpp.allowedJids}"
            "COMPANION_XMPP_MUC_ROOMS=${
              lib.concatMapStringsSep "," (r: "${r.jid}/${r.nick}") cfg.channels.xmpp.mucRooms
            }"
            "COMPANION_XMPP_MENTION_ONLY=${if cfg.channels.xmpp.mentionOnly then "1" else "0"}"
            "COMPANION_XMPP_STREAM_MODE=${cfg.channels.xmpp.streamMode}"
          ];
        };

        Install = {
          WantedBy = [ "default.target" ];
        };
      };
    })

    (lib.mkIf cfg.tui.enable {
      home.packages = [ cfg.tui.package ];
    })
  ]);
}
