{
  description = "Claude-style bottom statusline for Grok Build";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          grok-statusline = pkgs.callPackage ./default.nix { };
        in {
          inherit grok-statusline;
          default = grok-statusline;
        });

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/grok-statusline";
        };
        install = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/grok-statusline";
        };
      });

      devShells = forAllSystems (system:
        let pkgs = nixpkgs.legacyPackages.${system};
        in {
          default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              rustc
              cargo
              rustfmt
              clippy
              pkg-config
            ];
          };
        });
    };
}
