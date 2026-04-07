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
          ];
        };

        Install = {
          WantedBy = [ "default.target" ];
        };
      };
    })
  ]);
}
