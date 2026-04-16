# cairn-companion home-manager module.
#
# Tier 0: Exposes `services.cairn-companion.*` options and, when enabled,
# installs a per-user `companion` wrapper binary that invokes `claude`
# with persona files, workspace directory, and any mcp-gateway config.
#
# Tier 1: Adds `services.cairn-companion.daemon.*` options. When
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
  cfg = config.services.cairn-companion;

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
  options.services.cairn-companion = {
    enable = lib.mkEnableOption "cairn-companion Tier 0 wrapper around Claude Code";

    package = lib.mkOption {
      type = lib.types.package;
      default = builtCompanion;
      defaultText = lib.literalMD ''
        a per-user build produced by `lib.buildCompanion` using the other
        `services.cairn-companion.*` options
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
        defaultText = lib.literalExpression "inputs.cairn-companion.packages.\${pkgs.system}.personaDefault";
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
      default = "${config.xdg.dataHome}/cairn-companion/workspace";
      defaultText = lib.literalExpression "\"\${config.xdg.dataHome}/cairn-companion/workspace\"";
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
        defaultText = lib.literalExpression "inputs.cairn-companion.packages.\${pkgs.system}.companion-core";
        description = ''
          The companion-core daemon package. Override only if you have a
          local fork or want to pin a specific build.
        '';
      };

      mcpGatewayPackage = lib.mkOption {
        type = lib.types.nullOr lib.types.package;
        default = pkgs.mcp-gateway or null;
        defaultText = lib.literalExpression "pkgs.mcp-gateway or null";
        description = ''
          The mcp-gateway package providing the `mcp-gw` CLI. The daemon
          calls `mcp-gw --json list` once at startup to build the
          Anonymous permission deny list from the live gateway tool
          registry, so mcp-gw must be on the systemd unit's PATH.

          Defaults to `pkgs.mcp-gateway` if available in the caller's
          nixpkgs (e.g. via the cairn distro overlay). When using
          cairn-companion without the cairn distro, set this explicitly:

            services.cairn-companion.daemon.mcpGatewayPackage =
              inputs.mcp-gateway.packages.''${pkgs.system}.default;
        '';
      };
    };

    cli = {
      enable = lib.mkEnableOption "companion CLI (Tier 1 Rust binary replacing the shell wrapper)";

      package = lib.mkOption {
        type = lib.types.package;
        default = self.packages.${pkgs.system}.companion-cli;
        defaultText = lib.literalExpression "inputs.cairn-companion.packages.\${pkgs.system}.companion-cli";
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
        defaultText = lib.literalExpression "inputs.cairn-companion.packages.\${pkgs.system}.companion-tui";
        description = ''
          The companion TUI dashboard package. Provides `companion-tui`
          binary for terminal-native daemon monitoring.
        '';
      };
    };

    spoke = {
      enable = lib.mkEnableOption ''
        cairn-companion spoke tools (Tier 2). Machine-local MCP tool
        servers — desktop notifications, screenshot, clipboard, journal,
        apps, Niri control, shell. Each tool is a short-lived stdio MCP
        server spawned per-call by mcp-gateway. No daemon-core dependency:
        spoke tools are useful at Tier 0 and above.

        Enabling any tool under `spoke.tools.<tool>.enable` auto-registers
        that tool with the local mcp-gateway via
        `services.mcp-gateway.servers.companion-<tool>`. The consuming
        config is expected to have the mcp-gateway home-manager module
        imported; without it, those option emissions will fail at
        evaluation time
      '';

      package = lib.mkOption {
        type = lib.types.package;
        default = self.packages.${pkgs.system}.companion-spoke-tools;
        defaultText = lib.literalExpression "inputs.cairn-companion.packages.\${pkgs.system}.companion-spoke-tools";
        description = ''
          The companion spoke-tools package. Provides one binary per
          tool under `$out/bin/companion-mcp-<tool>`, each wrapped with
          its own runtime PATH so the tool's shell-outs resolve
          regardless of the mcp-gateway unit's inherited PATH.
        '';
      };

      tools.notify.enable = lib.mkEnableOption ''
        the `notify` tool — desktop notifications via notify-send.
        Picked up by whichever freedesktop-compliant notification
        daemon is running on the user's Wayland session (mako, DMS,
        etc.). Fire-and-forget
      '';

      tools.screenshot.enable = lib.mkEnableOption ''
        the `screenshot` tool — full-display capture via grim.
        Returns the PNG as MCP ImageContent so multimodal-capable
        clients can describe or reason about what's on screen. Runs
        on the mcp-gateway host — captures that host's display
      '';

      tools.clipboard.enable = lib.mkEnableOption ''
        the `clipboard` tool — read and write the Wayland clipboard
        via wl-paste and wl-copy. Text only for this phase. Runs on
        the mcp-gateway host — reads/writes that host's clipboard
      '';
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

    channels.discord = {
      enable = lib.mkEnableOption "Discord channel adapter inside the companion daemon";

      botTokenFile = lib.mkOption {
        type = lib.types.path;
        description = ''
          Path to a file containing the Discord bot token (one line,
          no trailing newline). Compatible with agenix-managed secrets.
          Create a bot at https://discord.com/developers/applications
          and enable the Message Content privileged intent.
        '';
        example = lib.literalExpression "/run/agenix/discord-bot-token";
      };

      allowedUserIds = lib.mkOption {
        type = lib.types.listOf lib.types.int;
        default = [ ];
        description = ''
          Discord user IDs (snowflakes) granted Owner trust in DMs.
          Empty list means nobody is Owner — all DMs get Anonymous
          trust (no tool access). Guild messages are always Anonymous
          regardless of this setting.
        '';
        example = [ 123456789012345678 ];
      };

      mentionOnly = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          When true, the bot only responds in guild channels when
          @mentioned. DMs are always handled regardless of this
          setting. Defaulted to true because guild channels would
          otherwise burn tokens on every message.
        '';
      };

      streamMode = lib.mkOption {
        type = lib.types.enum [ "single_message" "multi_message" ];
        default = "single_message";
        description = ''
          How to render streaming responses.
          `single_message`: edit a single message in place as chunks
          arrive (the message appears to type itself).
          `multi_message`: collect full response, split at 2000-char
          boundaries, send each chunk as a separate message.
        '';
      };
    };

    channels.email = {
      enable = lib.mkEnableOption "email channel adapter inside the companion daemon";

      address = lib.mkOption {
        type = lib.types.str;
        description = ''
          The bot's own mail address — both the IMAP login username and
          the outbound `From:` address. The mailbox must already exist
          on the IMAP server; the daemon does not provision accounts.
        '';
        example = "bot@example.com";
      };

      displayName = lib.mkOption {
        type = lib.types.str;
        default = "";
        description = ''
          Display name for the outbound `From:` header. Empty string
          defaults to the local part of `address`.
        '';
        example = "Bot";
      };

      passwordFile = lib.mkOption {
        type = lib.types.path;
        description = ''
          Path to a file containing the bot's IMAP/SMTP password (one
          line, no trailing newline). Compatible with agenix-managed
          secrets. The same password is used for both IMAP and SMTP —
          servers that require separate auth are not supported.
        '';
        example = lib.literalExpression "/run/agenix/email-bot-password";
      };

      imapHost = lib.mkOption {
        type = lib.types.str;
        description = ''
          IMAP server hostname. Must serve TLS on the configured port
          with a publicly verifiable certificate (Mozilla CA bundle).
          Self-signed certs are not supported on the email channel —
          use a real cert.
        '';
        example = "imap.example.com";
      };

      imapPort = lib.mkOption {
        type = lib.types.port;
        default = 993;
        description = "IMAPS port. Default 993 covers nearly every public IMAP host.";
      };

      smtpHost = lib.mkOption {
        type = lib.types.str;
        description = ''
          SMTP submission server hostname. Same TLS requirements as
          imapHost — public CA bundle, no self-signed certs.
        '';
        example = "smtp.example.com";
      };

      smtpPort = lib.mkOption {
        type = lib.types.port;
        default = 465;
        description = ''
          SMTPS port (implicit TLS wrapper). Default 465 is the
          submissions port — STARTTLS on 587 is not currently
          supported by the adapter.
        '';
      };

      pollIntervalSecs = lib.mkOption {
        type = lib.types.ints.between 5 3600;
        default = 30;
        description = ''
          How often (seconds) to poll IMAP for unseen messages. Floored
          at 5 to avoid hammering the server. v1 of the adapter
          deliberately uses polling instead of IMAP IDLE — IDLE is a
          follow-up if latency turns out to matter for a low-traffic
          channel.
        '';
      };

      allowedSenders = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        description = ''
          Email addresses (case-insensitive) granted `Owner` trust on
          inbound mail. Senders not in this list are still processed,
          but at `Anonymous` trust — they get a tool-free conversational
          reply, same as XMPP MUC. An empty list means everyone is
          anonymous, which is a valid (if unusual) deployment.
        '';
        example = [ "alice@example.com" "bob@example.org" ];
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
          message = "services.cairn-companion.cli requires daemon.enable — the CLI talks to the daemon via D-Bus";
        }
        {
          assertion = cfg.tui.enable -> cfg.daemon.enable;
          message = "services.cairn-companion.tui requires daemon.enable — the TUI talks to the daemon via D-Bus";
        }
        {
          assertion = cfg.channels.telegram.enable -> cfg.daemon.enable;
          message = "services.cairn-companion.channels.telegram requires daemon.enable — the adapter runs inside the daemon";
        }
        {
          assertion = cfg.channels.xmpp.enable -> cfg.daemon.enable;
          message = "services.cairn-companion.channels.xmpp requires daemon.enable — the adapter runs inside the daemon";
        }
        {
          assertion = cfg.channels.discord.enable -> cfg.daemon.enable;
          message = "services.cairn-companion.channels.discord requires daemon.enable — the adapter runs inside the daemon";
        }
        {
          assertion = cfg.channels.email.enable -> cfg.daemon.enable;
          message = "services.cairn-companion.channels.email requires daemon.enable — the adapter runs inside the daemon";
        }
        {
          assertion = cfg.daemon.enable -> cfg.daemon.mcpGatewayPackage != null;
          message = ''
            services.cairn-companion.daemon.enable requires daemon.mcpGatewayPackage
            to be set. The daemon shells out to `mcp-gw --json list` once at
            startup to build the Anonymous channel permission deny list, and
            refuses to start if it can't find the binary.

            If you use cairn's nixpkgs overlay, pkgs.mcp-gateway is available
            and the default handles it. Otherwise set:

              services.cairn-companion.daemon.mcpGatewayPackage =
                inputs.mcp-gateway.packages.''${pkgs.system}.default;
          '';
        }
        {
          assertion = cfg.spoke.enable -> cfg.spoke.package != null;
          message = ''
            services.cairn-companion.spoke.enable requires spoke.package to
            be set to the companion-spoke-tools package. The default
            resolves to the flake's own build; null it out only if you are
            providing your own.
          '';
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
          Description = "cairn-companion daemon";
          Documentation = "https://github.com/kcalvelli/cairn-companion";
        };

        Service = {
          Type = "notify";
          ExecStart = "${cfg.daemon.package}/bin/companion-core";
          Restart = "on-failure";
          RestartSec = 5;
          TimeoutStopSec = 130;
          Environment = [
            "XDG_DATA_HOME=${config.xdg.dataHome}"
            # mcp-gw (from mcpGatewayPackage) is on PATH so companion-core
            # can shell out to `mcp-gw --json list` at startup to build
            # the Anonymous deny list. The assertion above guarantees
            # mcpGatewayPackage is set whenever daemon.enable is true.
            "PATH=${cfg.package}/bin:${cfg.daemon.package}/bin:${cfg.daemon.mcpGatewayPackage}/bin:/run/current-system/sw/bin"
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
          ] ++ lib.optionals cfg.channels.discord.enable [
            "COMPANION_DISCORD_ENABLE=1"
            "COMPANION_DISCORD_BOT_TOKEN_FILE=${cfg.channels.discord.botTokenFile}"
            "COMPANION_DISCORD_ALLOWED_USER_IDS=${lib.concatMapStringsSep "," toString cfg.channels.discord.allowedUserIds}"
            "COMPANION_DISCORD_MENTION_ONLY=${if cfg.channels.discord.mentionOnly then "1" else "0"}"
            "COMPANION_DISCORD_STREAM_MODE=${cfg.channels.discord.streamMode}"
          ] ++ lib.optionals cfg.channels.email.enable [
            "COMPANION_EMAIL_ENABLE=1"
            "COMPANION_EMAIL_ADDRESS=${cfg.channels.email.address}"
            "COMPANION_EMAIL_DISPLAY_NAME=${cfg.channels.email.displayName}"
            "COMPANION_EMAIL_PASSWORD_FILE=${cfg.channels.email.passwordFile}"
            "COMPANION_EMAIL_IMAP_HOST=${cfg.channels.email.imapHost}"
            "COMPANION_EMAIL_IMAP_PORT=${toString cfg.channels.email.imapPort}"
            "COMPANION_EMAIL_SMTP_HOST=${cfg.channels.email.smtpHost}"
            "COMPANION_EMAIL_SMTP_PORT=${toString cfg.channels.email.smtpPort}"
            "COMPANION_EMAIL_POLL_INTERVAL_SECS=${toString cfg.channels.email.pollIntervalSecs}"
            "COMPANION_EMAIL_ALLOWED_SENDERS=${lib.concatStringsSep "," cfg.channels.email.allowedSenders}"
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

    # Spoke tools — Tier 2 MCP tool servers.
    #
    # Each enabled tool is registered as an mcp-gateway stdio server.
    # The consumer must have the mcp-gateway home-manager module
    # imported; otherwise these option emissions fail at eval time
    # with "unknown option services.mcp-gateway.servers.*", which is
    # the right error to surface.
    (lib.mkIf (cfg.spoke.enable && cfg.spoke.tools.notify.enable) {
      services.mcp-gateway.servers.companion-notify = {
        enable = true;
        command = "${cfg.spoke.package}/bin/companion-mcp-notify";
      };
    })

    (lib.mkIf (cfg.spoke.enable && cfg.spoke.tools.screenshot.enable) {
      services.mcp-gateway.servers.companion-screenshot = {
        enable = true;
        command = "${cfg.spoke.package}/bin/companion-mcp-screenshot";
      };
    })

    (lib.mkIf (cfg.spoke.enable && cfg.spoke.tools.clipboard.enable) {
      services.mcp-gateway.servers.companion-clipboard = {
        enable = true;
        command = "${cfg.spoke.package}/bin/companion-mcp-clipboard";
      };
    })
  ]);
}
