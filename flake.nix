{
  description = "FabricO11y Linux observability and durable telemetry custody";

  inputs = {
    # Development pin for NixOS 26.11-pre. Release acceptance requires nixos-26.11.
    nixpkgs.url = "github:NixOS/nixpkgs/04906337b6d53e8bb0eaf0470ae9f070a4d4eb55";
    nixpkgs-26_05.url = "github:NixOS/nixpkgs/nixos-26.05";
    rust-overlay = {
      url = "github:oxalica/rust-overlay/6b05b928a40211beecaaa294e6f719aa2145a2a8";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      nixpkgs-26_05,
      rust-overlay,
      ...
    }:
    let
      system = "x86_64-linux";
      forRelease =
        nixpkgs: stateVersion:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          package = pkgs.callPackage ./packaging/nix/package.nix { };
          module = import ./packaging/nix/module.nix { inherit package; };
        in
        {
          inherit package module;
          app = {
            type = "app";
            program = "${package}/bin/fabric";
            meta.description = package.meta.description;
          };
          check =
            assert import ./packaging/nix/test-module-paths.nix {
              inherit module system stateVersion;
              lib = nixpkgs.lib;
            };
            import ./packaging/nix/test.nix { inherit pkgs module stateVersion; };
        };
      current = forRelease nixpkgs "26.11";
      compatibility = forRelease nixpkgs-26_05 "26.05";
    in
    {
      packages.${system} = {
        default = current.package;
        fabrico11y = current.package;
        fabrico11y-26_05 = compatibility.package;
      };

      apps.${system} = {
        default = current.app;
        fabrico11y = current.app;
        fabrico11y-26_05 = compatibility.app;
      };

      nixosModules = {
        fabrico11y = current.module;
        fabrico11y-26_05 = compatibility.module;
      };

      checks.${system} = {
        nixos = current.check;
        nixos-26_05 = compatibility.check;
      };
    };
}
