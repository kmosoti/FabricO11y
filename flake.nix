{
  description = "FabricO11y Linux observability and durable telemetry custody";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    rust-overlay = {
      url = "github:oxalica/rust-overlay/6b05b928a40211beecaaa294e6f719aa2145a2a8";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      system = "x86_64-linux";
      stateVersion = "26.05";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      };
      package = pkgs.callPackage ./packaging/nix/package.nix { };
      module = import ./packaging/nix/module.nix { inherit package; };
      app = {
        type = "app";
        program = "${package}/bin/fabric";
        meta.description = package.meta.description;
      };
    in
    {
      packages.${system} = {
        default = package;
        fabrico11y = package;
        fabrico11y-26_05 = package;
      };

      apps.${system} = {
        default = app;
        fabrico11y = app;
        fabrico11y-26_05 = app;
      };

      nixosModules = {
        fabrico11y = module;
        fabrico11y-26_05 = module;
      };

      checks.${system}.nixos =
        assert import ./packaging/nix/test-module-paths.nix {
          inherit module system stateVersion;
          lib = nixpkgs.lib;
        };
        import ./packaging/nix/test.nix { inherit pkgs module stateVersion; };
    };
}
