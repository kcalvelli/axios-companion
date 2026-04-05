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
    in
    {
      # Overlay - will be populated once the bootstrap change adds a package
      # See openspec/changes/bootstrap/
      overlays.default = final: prev: { };

      # Home-Manager Module - will be populated by the bootstrap change
      # See openspec/changes/bootstrap/specs/home-manager/spec.md
      homeManagerModules.default = _: { };

      # Dev shell - available now for working on openspec proposals
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
