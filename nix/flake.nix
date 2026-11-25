{
  description = "AI-Native OS - Self-modifying, federated, minimal";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.11";
    # Optional private flake for secrets and host-specific configs
    # private = {
    #   url = "git+ssh://git@github.com/your-org/ai-native-os-private";
    #   flake = false;
    # };
  };

  outputs = { self, nixpkgs, ... }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs supportedSystems (system: f system);

      # Common modules
      commonModules = [
        ./modules/kernel.nix
        ./modules/inference.nix
        ./modules/sandbox.nix
        ./modules/federation.nix
        ./modules/orchestrator.nix
        ./modules/hardening.nix
      ];
    in
    {
      nixosConfigurations = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; config = { allowUnfree = true; }; };
          hostName = builtins.getAttr "hostname" (pkgs.lib.getSystem system);
          # Optional private host config
          # hostConfig = private.nixosConfigurations.${hostName} or { };
          hostConfig = { };
        in
        nixpkgs.lib.nixosSystem {
          inherit system;
          specialArgs = { inherit pkgs; };
          modules = commonModules ++ [ hostConfig ];
        }
      );

      # Development shell
      devShells = forAllSystems (system:
        let pkgs = import nixpkgs { inherit system; config = { allowUnfree = true; }; };
        in pkgs.mkShell {
          buildInputs = with pkgs; [
            rustup
            clang
            lld
            bpftool
            llvmPackages_18
            nix
            firecracker
            qemu_kvm
            python3
            python3Packages.pytest
            linuxPackages.perf
            linuxPackages.kernel-tools
            cargo-audit
            cargo-nextest
            taplo
            nil
          ];
          RUSTUP_TOOLCHAIN = "stable";
          SHELL_HOOK = ''
            export RUSTUP_TOOLCHAIN=stable
            source $stdenv/setup
          '';
        }
      );

      # Packages
      packages = forAllSystems (system:
        let pkgs = import nixpkgs { inherit system; config = { allowUnfree = true; }; };
        in {
          orchestrator = pkgs.callPackage ./packages/orchestrator { };
        }
      );

      # Host configurations as explicit outputs
      nixosConfigurations.laptop = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = commonModules ++ [ ./hosts/laptop.nix ];
      };

      nixosConfigurations.phone = nixpkgs.lib.nixosSystem {
        system = "aarch64-linux";
        modules = commonModules ++ [ ./hosts/phone.nix ];
      };
    };
}