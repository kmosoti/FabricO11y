{
  description = "FabricO11y Linux observability and durable telemetry custody";

  inputs = {
    # Development pin for NixOS 26.11-pre. Release acceptance requires nixos-26.11.
    nixpkgs.url = "github:NixOS/nixpkgs/04906337b6d53e8bb0eaf0470ae9f070a4d4eb55";
    rust-overlay = {
      url = "github:oxalica/rust-overlay/6b05b928a40211beecaaa294e6f719aa2145a2a8";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { self, nixpkgs, rust-overlay }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      };
      fabric = pkgs.callPackage ./packaging/nix/package.nix { };
      app = {
        type = "app";
        program = "${fabric}/bin/fabric";
        meta.description = fabric.meta.description;
      };
    in
    {
      packages.${system} = {
        fabrico11y = fabric;
        default = fabric;
      };

      apps.${system} = {
        fabrico11y = app;
        default = app;
      };

      nixosModules.fabrico11y = import ./packaging/nix/module.nix { package = fabric; };

      checks.${system}.nixos =
        let
          module = self.nixosModules.fabrico11y;
        in
        assert import ./packaging/nix/test-module-paths.nix {
          inherit module system;
          lib = nixpkgs.lib;
        };
        import ./packaging/nix/test.nix { inherit pkgs module; };
    };
}
