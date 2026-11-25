{ config, pkgs, lib, ... }:
{
  # Host metadata
  networking.hostName = "phone";
  networking.domain = "local";
  
  # Mobile-specific boot
  boot.kernelParams = [ "androidboot.hardware=qcom" "console=ttyMSM0,115200n8" "androidboot.console=ttyMSM0" "androidboot.usbcontroller=a600000.dwc3" ];
  
  # Hardware enablement (Qualcomm Snapdragon)
  hardware = {
    # GPU (Adreno)
    opengl = {
      enable = true;
      driSupport = true;
      driSupport32Bit = true;
    };
    vulkan = { enable = true; };
    
    # NPU (Qualcomm QNN)
    npu = {
      enable = true;
      # QNN SDK path would be configured here
    };
    
    # Sensors
    sensors = {
      enable = true;
      iio = true;
    };
  };
  
  # Orchestrator config (lighter for mobile)
  orchestrator = {
    enable = true;
    deviceId = "phone";
    configFile = "/etc/orchestrator/config.toml";
    
    kernel = {
      enable = true;
      version = "6.6";
      modules = [ "wcn3990" "qcom_smem" "qcom_rpm" "msm_drm" "adreno" ];
      # Mobile-specific kernel config would go here
    };
    
    inference = {
      enable = true;
      modelPath = "/var/lib/orchestrator/models/qwen2.5-coder-7b-4bit.gguf";  # Smaller model for phone
      loraPath = "/var/lib/orchestrator/lora";
      ragPath = "/var/lib/orchestrator/rag";
      backend = "candle";
      gpu = true;
      npu = true;
      quantization = "Q4_K_M";
      contextLength = 16384;  # Smaller context for mobile
      gpuLayers = -1;
      idleTimeout = 60;  # Longer idle timeout on battery
    };
    
    sandbox = {
      enable = true;
      backend = "firecracker";
      firecracker = {
        vcpus = 2;
        memoryMiB = 512;
      };
      warmPoolSize = 0;
    };
    
    federation = {
      enable = true;
      tailscaleAuthKey = null;
      syncInterval = 600;  # Less frequent on mobile
      automergePath = "/var/lib/orchestrator/federation.automerge";
      gitRemote = "git@github.com:your-org/ai-native-os-federation.git";
      gitBranch = "main";
    };
    
    ui = {
      mode = "web";  # Web UI better for mobile
      webPort = 8080;
      webBind = "0.0.0.0";  # Accessible from laptop
    };
    
    hardening = {
      enable = true;
      kernelLockdown = "confidentiality";
      moduleSigEnforce = true;
    };
  };
  
  # Mobile networking
  networking = {
    networkmanager.enable = true;
    wireless.enable = true;
    wireless.iwd.enable = true;
    firewall.enable = true;
    firewall.allowedTCPPorts = [ 8080 ];  # Orchestrator web UI
  };
  
  # Power management
  services.tlp.enable = true;
  powerManagement.cpuFreqGovernor = "powersave";
  
  # Android container (Waydroid) for app compatibility
  virtualisation.waydroid = {
    enable = true;
    package = pkgs.waydroid;
  };
  
  # Minimal packages for mobile
  environment.systemPackages = with pkgs; [
    git curl wget
    tailscale
    firecracker
    nix-index
  ];
}