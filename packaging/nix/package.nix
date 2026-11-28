{
  lib,
  makeRustPlatform,
  python3,
  rust-bin,
}:
let
  toolchain = rust-bin.fromRustupToolchainFile ../../rust-toolchain.toml;
  rustPlatform = makeRustPlatform {
    cargo = toolchain;
    rustc = toolchain;
  };
  binaries = [
    "fabric"
    "spindle"
    "fabric-server"
    "fabric-adapter"
    "fabric-query"
    "fabric-mcp"
  ];
in
rustPlatform.buildRustPackage {
  pname = "fabrico11y";
  version = "0.2.0-rc.1";
  src = lib.cleanSource ../..;
  cargoLock.lockFile = ../../Cargo.lock;

  # Keep the workspace's bundled SQLite and Zstandard sources. The Nixpkgs
  # Cargo hook otherwise forces pkg-config to replace them with system libraries.
  dontCargoUseSystemLibs = true;
  nativeBuildInputs = [ python3 ];
  cargoBuildFlags = [ "--workspace" "--locked" ] ++ lib.concatMap (bin: [ "--bin" bin ]) binaries;

  # Host custody and service tests run in the NixOS VM and existing product suites.
  doCheck = false;
  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck
    for bin in ${lib.escapeShellArgs binaries}; do
      test -x "$out/bin/$bin"
    done
    "$out/bin/fabric" --version
    runHook postInstallCheck
  '';

  postInstall = ''
    licenseDir="$out/share/licenses/fabrico11y"
    install -Dm644 -t "$licenseDir" LICENSE NOTICE
    python3 packaging/dependency-notices.py "$licenseDir/dependencies"
    install -Dm644 -t "$out/share/fabrico11y/config" config/*.toml
  '';

  meta = {
    description = "CLI-first Linux host and agent observability with durable telemetry custody";
    homepage = "https://github.com/kmosoti/FabricO11y";
    license = lib.licenses.asl20;
    mainProgram = "fabric";
    platforms = [ "x86_64-linux" ];
  };
}
