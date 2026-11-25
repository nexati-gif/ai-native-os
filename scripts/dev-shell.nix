{ pkgs ? import <nixpkgs> { config = { allowUnfree = true; }; } }:

pkgs.mkShell {
  name = "ai-native-os-dev";
  
  buildInputs = with pkgs; [
    # Rust toolchain
    rustup
    cargo
    rust-analyzer
    cargo-audit
    cargo-nextest
    cargo-tarpaulin
    cargo-hakari
    taplo
    
    # C/C++ toolchain for eBPF
    clang
    lld
    llvmPackages_17
    bpftool
    linuxPackages.perf
    linuxPackages.kernel-tools
    libbpf
    
    # Nix
    nix
    nil
    nix-index
    nixpkgs-fmt
    
    # Virtualization
    firecracker
    qemu_kvm
    virt-manager
    
    # Testing
    python3
    python3Packages.pytest
    python3Packages.pytest-asyncio
    python3Packages.hypothesis
    
    # Utilities
    git
    jq
    yq
    htop
    btop
    fd
    ripgrep
    bat
    eza
    zoxide
    direnv
    just
    watchman
    
    # Containers
    podman
    buildah
    skopeo
    
    # Documentation
    mdbook
    mdbook-linkcheck
  ];
  
  # Rust environment
  RUSTUP_TOOLCHAIN = "stable";
  CARGO_TARGET_DIR = "./target";
  
  # Shell hook
  shellHook = ''
    # Use direnv if available
    if command -v direnv &>/dev/null; then
      eval "$(direnv hook bash)"
    fi
    
    # Rust setup
    export RUSTUP_TOOLCHAIN=stable
    export CARGO_TARGET_DIR=./target
    export RUSTFLAGS="-C target-cpu=native -C opt-level=3"
    
    # Cargo aliases
    alias cb='cargo build --workspace'
    alias ct='cargo test --workspace'
    alias ccl='cargo clippy --workspace -- -D warnings'
    alias cf='cargo fmt --all'
    alias ca='cargo audit'
    alias cn='cargo nextest run'
    
    # Nix aliases
    alias nr='nixos-rebuild switch --flake .#$(hostname)'
    alias nb='nix build .#orchestrator'
    alias nf='nix flake check'
    
    # Project aliases
    alias or='./target/release/orchestrator'
    alias ort='./target/release/orchestrator-tui'
    
    echo "🤖 AI-Native OS Development Environment"
    echo "  cb   - cargo build"
    echo "  ct   - cargo test"
    echo "  ccl  - cargo clippy"
    echo "  cf   - cargo fmt"
    echo "  nr   - nixos-rebuild"
    echo "  or   - run orchestrator"
  '';
  
  # Native build inputs for native code
  nativeBuildInputs = with pkgs; [
    pkg-config
    cmake
    make
    automake
    autoconf
    libtool
  ];
}