{
  description = "axios-companion - A persistent, customizable persona wrapper around Claude Code";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;

      # claude-code is marked unfree in nixpkgs. Import a per-system pkgs
      # with a narrow predicate allowing just that package, so the
      # reference build can be produced with `nix build .#default`.
      # Consumers of `lib.<system>.buildCompanion` are free to pass any
      # claudePackage they like and are not bound by this predicate.
      pkgsFor =
        system:
        import nixpkgs {
          inherit system;
          config.allowUnfreePredicate =
            pkg: builtins.elem (nixpkgs.lib.getName pkg) [ "claude-code" ];
        };
    in
    {
      # Overlay — exposes only the reference `axios-companion` wrapper build.
      # The default-persona package is implementation detail and deliberately
      # not promoted to overlay status.
      overlays.default = final: prev: {
        axios-companion = self.packages.${final.system}.default;
      };

      # Home-Manager Module — exposes `services.axios-companion.*`. Imports
      # as a closure over `self` so the module can reach
      # `self.lib.${pkgs.system}.buildCompanion` when building the per-user
      # wrapper. See openspec/changes/bootstrap/specs/home-manager/spec.md.
      homeManagerModules.default = import ./modules/home-manager { inherit self; };

      # Packages.
      #
      #   .default         — the reference companion build, using
      #                      `buildCompanion` with default arguments. Suitable
      #                      for `nix build` smoke testing and documentation.
      #                      NOT the user-facing build path — real per-user
      #                      builds go through the home-manager module, which
      #                      calls `lib.<system>.buildCompanion` directly.
      #
      #   .personaDefault  — the character-free default persona package.
      #                      Exposed by name for advanced users who want to
      #                      reference it explicitly. Not included in the
      #                      overlay.
      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          personaDefault = pkgs.callPackage ./packages/persona-default { };
          referenceCompanion = self.lib.${system}.buildCompanion {
            claudePackage = pkgs.claude-code;
            personaBasePackage = personaDefault;
            defaultWorkspace = "__HOME__/.local/share/axios-companion/workspace";
          };
        in
        {
          default = referenceCompanion;
          personaDefault = personaDefault;
          companion-core = pkgs.callPackage ./packages/companion-core { };
        }
      );

      # Flake-exposed build helper. This is the public contract the
      # home-manager module consumes — see specs/home-manager/spec.md
      # "Module Builds The Wrapper Via `lib.buildCompanion`".
      #
      # Accepted arguments:
      #   claudePackage       — the claude-code package to invoke
      #   personaBasePackage  — package containing AGENT.md and USER.md
      #   defaultWorkspace    — string path baked into the wrapper
      #   userFile            — nullable path; overrides base USER.md
      #   extraFiles          — list of additional persona files, appended
      #   mcpConfigFile       — nullable path; overrides MCP auto-detection
      lib = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
        in
        {
          buildCompanion = args: pkgs.callPackage ./packages/companion args;
        }
      );

      # Dev shell.
      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              nixfmt-rfc-style
              git
              gh
              cargo
              rustc
              rust-analyzer
              clippy
              rustfmt
            ];

            shellHook = ''
              echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
              echo "  axios-companion development environment"
              echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
              echo ""
              echo "Roadmap:    cat ROADMAP.md"
              echo "Proposals:  ls openspec/changes/"
              echo "Next up:    openspec/changes/bootstrap/"
            '';
          };
        }
      );
    };
}
