{ config, pkgs, lib, ... }:
{
  # Host metadata
  networking.hostName = "laptop";
  networking.domain = "local";
  
  # Boot
  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;
  
  # Hardware enablement (adjust for your laptop)
  hardware.enableAllFirmware = true;
  hardware.intel-ucode.enable = true;
  hardware.nvidia = {
    modesetting.enable = true;
    powerManagement = true;
    powerManagementFinegrained = true;
  };
  
  # Orchestrator config
  orchestrator = {
    enable = true;
    deviceId = "laptop";
    configFile = "/etc/orchestrator/config.toml";
    
    kernel = {
      enable = true;
      version = "6.6";
      modules = [ "nvidia" "nvidia_modeset" "nvidia_uvm" "nvidia_drm" ];
    };
    
    inference = {
      enable = true;
      modelPath = "/var/lib/orchestrator/models/qwen2.5-coder-32b-4bit.gguf";
      loraPath = "/var/lib/orchestrator/lora";
      ragPath = "/var/lib/orchestrator/rag";
      backend = "candle";
      gpu = true;
      quantization = "Q4_K_M";
      contextLength = 32768;
      gpuLayers = -1;
      idleTimeout = 30;
    };
    
    sandbox = {
      enable = true;
      backend = "firecracker";
      firecracker = {
        vcpus = 4;
        memoryMiB = 1024;
      };
      warmPoolSize = 0;
    };
    
    federation = {
      enable = true;
      tailscaleAuthKey = null;  # Set via environment
      syncInterval = 300;
      automergePath = "/var/lib/orchestrator/federation.automerge";
      gitRemote = "git@github.com:your-org/ai-native-os-federation.git";
      gitBranch = "main";
    };
    
    ui = {
      mode = "both";
      webPort = 8080;
      webBind = "127.0.0.1";
    };
    
    hardening = {
      enable = true;
      kernelLockdown = "confidentiality";
      moduleSigEnforce = true;
    };
  };
  
  # Users
  users.users.youruser = {
    isNormalUser = true;
    extraGroups = [ "wheel" "orchestrator" "docker" "libvirt" "kvm" ];
    packages = with pkgs; [
      git vim neovim htop btop
      firefox chromium
      vscode
    ];
  };
  
  # Services
  services.docker.enable = true;
  services.libvirtd.enable = true;
  
  # Fonts
  fonts.fonts = with pkgs; [ noto-fonts noto-fonts-cjk noto-fonts-emoji jetbrains-mono ];
  
  # System packages
  environment.systemPackages = with pkgs; [
    git curl wget htop btop jq yq
    firecracker qemu_kvm
    tailscale
    nix-index
    cargo-audit cargo-nextest
  ];
}