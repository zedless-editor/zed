{
  description = ''
    High-performance, multiplayer code editor from the creators of Atom and Tree-sitter
  '';

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs?ref=nixos-unstable";
    flake-compat.url = "github:edolstra/flake-compat";
  };

  outputs = {
    self,
    nixpkgs,
    ...
  }: let
    systems = [
      "x86_64-linux"
      "x86_64-darwin"
      "aarch64-linux"
      "aarch64-darwin"
    ];

    forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f (nixpkgs.legacyPackages.${system}));
  in {
    packages = forAllSystems (pkgs: {
      zed-editor = pkgs.callPackage ./nix/package.nix {};
      default = self.packages.${pkgs.stdenv.system}.zed-editor;
    });

    devShells = forAllSystems (pkgs: {
      default = pkgs.callPackage ./nix/shell.nix {};
    });

    formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
  };
}
