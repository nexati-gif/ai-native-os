{ config, pkgs, lib, ... }:
let
  cfg = config.orchestrator.inference;
in {
  options.orchestrator.inference = {
    enable = lib.mkEnableOption "Local LLM inference for orchestrator";
    modelPath = lib.mkOption {
      type = lib.types.path;
      description = "Path to model weights (GGUF)";
    };
    loraPath = lib.mkOption {
      type = lib.types.path;
      description = "Path to LoRA adapters";
    };
    ragPath = lib.mkOption {
      type = lib.types.path;
      description = "Path to RAG index";
    };
    backend = lib.mkOption {
      type = lib.types.enum [ "candle" "llama.cpp" "vllm" ];
      default = "candle";
      description = "Inference backend";
    };
    gpu = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Enable GPU acceleration";
    };
    npu = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Enable NPU acceleration (Qualcomm/Google/Apple)";
    };
    quantization = lib.mkOption {
      type = lib.types.enum [ "Q4_K_M" "Q4_K_S" "Q5_K_M" "Q5_K_S" "Q6_K" "Q8_0" "F16" ];
      default = "Q4_K_M";
      description = "Model quantization";
    };
    contextLength = lib.mkOption {
      type = lib.types.int;
      default = 32768;
      description = "Context window size";
    };
    gpuLayers = lib.mkOption {
      type = lib.types.int;
      default = -1;
      description = "Number of layers on GPU (-1 = all)";
    };
    threads = lib.mkOption {
      type = lib.types.int;
      default = 0;
      description = "CPU threads (0 = auto)";
    };
    idleTimeout = lib.mkOption {
      type = lib.types.int;
      default = 30;
      description = "Idle timeout in seconds before unload";
    };
  };
  
  config = lib.mkIf cfg.enable {
    # Model weights directory
    systemd.tmpfiles.rules = [
      "d /var/lib/orchestrator/models 0755 orchestrator orchestrator -"
      "d /var/lib/orchestrator/lora 0755 orchestrator orchestrator -"
      "d /var/lib/orchestrator/rag 0755 orchestrator orchestrator -"
    ];
    
    # GPU/NPU drivers
    hardware.opengl.enable = lib.mkForce cfg.gpu;
    hardware.nvidia.modesetting.enable = lib.mkForce (cfg.gpu && config.boot.kernelPackages.config.NVIDIA != null);
    
    # Vulkan for compute
    hardware.vulkan.enable = lib.mkForce cfg.gpu;
    
    # NPU support (Qualcomm)
    hardware.npu = {
      enable = lib.mkForce cfg.npu;
      # Qualcomm QNN SDK would go here
    };
    
    # Download model if not present
    systemd.services.download-model = {
      description = "Download orchestrator model";
      wantedBy = [ "multi-user.target" ];
      before = [ "orchestrator.service" ];
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${pkgs.curl}/bin/curl -L -o ${cfg.modelPath} ${cfg.modelUrl}";
        RemainAfterExit = true;
        User = "orchestrator";
      };
    };
    
    # Environment variables for orchestrator
    systemd.services.orchestrator.environment = {
      ORCHESTRATOR_MODEL_PATH = cfg.modelPath;
      ORCHESTRATOR_LORA_PATH = cfg.loraPath;
      ORCHESTRATOR_RAG_PATH = cfg.ragPath;
      ORCHESTRATOR_INFERENCE_BACKEND = cfg.backend;
      ORCHESTRATOR_QUANTIZATION = cfg.quantization;
      ORCHESTRATOR_CONTEXT_LENGTH = toString cfg.contextLength;
      ORCHESTRATOR_GPU_LAYERS = toString cfg.gpuLayers;
      ORCHESTRATOR_THREADS = toString (if cfg.threads == 0 then lib.getCores else cfg.threads);
      ORCHESTRATOR_IDLE_TIMEOUT = toString cfg.idleTimeout;
    };
  };
}