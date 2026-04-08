# axios-companion Tier 0 wrapper — writeShellApplication factory.
#
# This is the function exposed as `lib.<system>.buildCompanion` on the flake.
# Callers (notably the home-manager module in modules/home-manager/) invoke
# it with their resolved options; the result is a package containing a
# `companion` binary ready to drop into `home.packages`.
#
# All persona paths, the HAS_USER_FILE flag, the default workspace, and any
# explicit mcpConfigFile are baked into the generated script at Nix eval
# time. Reading the script reveals exactly which files will be loaded; no
# runtime directory scanning or env-var lookups are used for configuration.
# See specs/wrapper/spec.md for the authoritative behavior.
{
  lib,
  writeShellApplication,
  coreutils,
  claudePackage,
  personaBasePackage,
  defaultWorkspace,
  userFile ? null,
  extraFiles ? [ ],
  mcpConfigFile ? null,
}:
let
  agentPath = "${personaBasePackage}/AGENT.md";
  baseUserPath = "${personaBasePackage}/USER.md";

  # Persona resolution order, per specs/wrapper/spec.md:
  #   1. base AGENT.md
  #   2. user file (if set) OR base USER.md template
  #   3. each extraFile in order
  #
  # IMPORTANT: use "${...}" interpolation, NOT toString. `toString` strips
  # Nix string context, which prevents the output reference scanner from
  # registering the source derivation these paths live in as a runtime
  # dependency of the wrapper. Without that dependency, the source gets
  # garbage-collected between rebuilds and the baked persona paths point
  # at deleted files — emitting "persona file not found" warnings until
  # the next rebuild-switch reimports the source. Don't undo this in a
  # cleanup pass.
  userPath = if userFile != null then "${userFile}" else baseUserPath;
  personaPaths = [ agentPath userPath ] ++ map (p: "${p}") extraFiles;

  # Manual single-quote escape that preserves Nix string context.
  # Do NOT replace this with lib.escapeShellArg — it calls toString
  # internally (see nixpkgs lib/strings.nix), which undoes the context
  # preservation above and reintroduces the GC bug.
  escapeArg = s: "'" + builtins.replaceStrings [ "'" ] [ "'\\''" ] s + "'";

  # Bash array body: one escaped path per line.
  personaArrayBody = lib.concatMapStringsSep "\n" (p: "  ${escapeArg p}") personaPaths;

  hasUserFile = if userFile != null then "1" else "0";
  mcpExplicit = if mcpConfigFile != null then toString mcpConfigFile else "";
in
writeShellApplication {
  name = "companion";
  runtimeInputs = [
    coreutils
    claudePackage
  ];
  text = ''
    # axios-companion Tier 0 wrapper (generated).
    #
    # Everything below the "Build-time-baked configuration" block is generic
    # shell logic; the values in that block are baked by Nix at build time.
    # If you want to see exactly which persona files this binary loads,
    # read the PERSONA_PATHS array below.

    # --- Build-time-baked configuration -------------------------------------

    HAS_USER_FILE=${hasUserFile}
    DEFAULT_WORKSPACE=${lib.escapeShellArg defaultWorkspace}
    BASE_USER_TEMPLATE=${lib.escapeShellArg baseUserPath}
    MCP_EXPLICIT=${lib.escapeShellArg mcpExplicit}

    PERSONA_PATHS=(
    ${personaArrayBody}
    )

    # --- Workspace path expansion -------------------------------------------

    # Home-manager callers pass an absolute path, so the substitution below
    # is a no-op for them. The reference build (nix build) bakes a literal
    # "__HOME__" sentinel which we expand here so smoke-testing works
    # without a home-manager configuration. A sentinel is used instead of
    # "~" because tildes don't expand inside quoted assignments.
    WORKSPACE="''${DEFAULT_WORKSPACE//__HOME__/$HOME}"

    # --- First-run scaffolding ----------------------------------------------

    if [ ! -d "$WORKSPACE" ]; then
      mkdir -p "$WORKSPACE"
      cat > "$WORKSPACE/README.md" <<'EOF'
    # axios-companion workspace

    This directory is your companion's home on the filesystem. It is
    attached to every `companion` invocation via `claude --add-dir`, so
    anything you put here is readable (and writable) by the agent.

    Suggested uses:

    - Long-lived notes and reference material the agent should know about
    - Project bookmarks, task lists, or memory files the agent maintains
    - Personal context that belongs on this machine

    This directory is not synchronized across machines by axios-companion.
    If you want it to follow you between machines, sync it yourself
    (git, syncthing, Tailscale Drive, etc.).
    EOF

      if [ "$HAS_USER_FILE" = "0" ]; then
        cp "$BASE_USER_TEMPLATE" "$WORKSPACE/USER.md"
        chmod u+w "$WORKSPACE/USER.md"
      fi
    fi

    # --- Assemble system prompt from baked persona paths --------------------

    PERSONA=""
    for path in "''${PERSONA_PATHS[@]}"; do
      if [ -f "$path" ]; then
        if [ -n "$PERSONA" ]; then
          PERSONA="$PERSONA"$'\n\n'
        fi
        PERSONA="$PERSONA$(cat "$path")"
      else
        echo "companion: warning: persona file not found: $path" >&2
      fi
    done

    # --- MCP config resolution ----------------------------------------------
    # If mcpConfigFile was set explicitly at build time, use it (warn if
    # missing, still invoke claude). Otherwise auto-detect in the order
    # documented in specs/wrapper/spec.md.

    MCP_CONFIG=""
    if [ -n "$MCP_EXPLICIT" ]; then
      if [ -f "$MCP_EXPLICIT" ]; then
        MCP_CONFIG="$MCP_EXPLICIT"
      else
        echo "companion: warning: mcpConfigFile set but not found: $MCP_EXPLICIT" >&2
      fi
    else
      for candidate in \
        "''${XDG_CONFIG_HOME:-$HOME/.config}/mcp-gateway/claude_config.json" \
        "''${XDG_CONFIG_HOME:-$HOME/.config}/mcp/mcp_servers.json" \
        "$HOME/.mcp.json"
      do
        if [ -f "$candidate" ]; then
          MCP_CONFIG="$candidate"
          break
        fi
      done
    fi

    # --- Invoke claude ------------------------------------------------------

    args=(
      --append-system-prompt "$PERSONA"
      --add-dir "$WORKSPACE"
    )
    # Use the --flag=value form for --mcp-config. Claude Code's argparse
    # treats --mcp-config as nargs='+' (accepts multiple space-separated
    # paths), which means a bare `--mcp-config /path Hello` gets parsed as
    # TWO MCP config files — the real path and the user's positional
    # prompt. The equals form binds exactly one value and prevents the
    # positional from being consumed.
    if [ -n "$MCP_CONFIG" ]; then
      args+=("--mcp-config=$MCP_CONFIG")
    fi

    exec claude "''${args[@]}" "$@"
  '';
}
