{ pkgs, ... }:
let
  # Rust toolchain
  rustToolchain = pkgs.rust-bin.stable.latest.default;
in
pkgs.stdenv.mkDerivation {
  name = "orchestrator";
  version = "0.1.0";
  
  # Use the workspace root as source
  src = pkgs.lib.cleanSourceWith {
    src = ./.;
    filter = path: type:
      ! (path == ".git" || path == "target" || path == ".direnv");
  } ./.;
  
  nativeBuildInputs = [
    rustToolchain
    pkgs.cargo
    pkgs.pkg-config
    pkgs.clang
    pkgs.llvmPackages_18
    pkgs.openssl
    pkgs.libgit2
    pkgs.protobuf
  ];
  
  buildInputs = [
    pkgs.openssl
    pkgs.libgit2
    pkgs.zstd
    pkgs.sqlite
    pkgs.libseccomp
    pkgs.elfutils
  ];
  
  # Build flags
  CARGO_HOME = "${pwd}/.cargo-home";
  RUSTUP_HOME = "${pwd}/.rustup-home";
  
  # Build phases
  buildPhase = ''
    export CARGO_HOME="${CARGO_HOME}"
    export RUSTUP_HOME="${RUSTUP_HOME}"
    export RUSTFLAGS="-C target-cpu=native -C opt-level=3 -C lto=thin -C codegen-units=1 -C panic=abort"
    
    # Build the workspace
    cargo build --release --workspace --locked
  '';
  
  installPhase = ''
    mkdir -p $out/bin
    cp target/release/orchestrator $out/bin/
    
    # Install config example
    mkdir -p $out/share/orchestrator
    cp config.example.toml $out/share/orchestrator/ 2>/dev/null || true
    
    # Install policies
    mkdir -p $out/share/orchestrator/policies
    cp -r nix/modules/orchestrator.nix $out/share/orchestrator/policies/ 2>/dev/null || true
  '';
  
  # Check phase (run only unit tests, skip integration)
  checkPhase = ''
    cargo test --workspace --locked --lib
  '';
  
  meta = with pkgs.lib; {
    description = "AI-Native OS Orchestrator";
    license = licenses.apache-2-0;
    platforms = platforms.linux;
    maintainers = with maintainers; [ ];
  };
}