{ config, pkgs, lib, ... }:
let
  cfg = config.orchestrator.federation;
in {
  options.orchestrator.federation = {
    enable = lib.mkEnableOption "Cross-device federation";
    tailscaleAuthKey = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Tailscale auth key";
    };
    syncInterval = lib.mkOption {
      type = lib.types.int;
      default = 300;
      description = "Sync interval in seconds";
    };
    automergePath = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/orchestrator/federation.automerge";
      description = "Automerge document path";
    };
    gitRemote = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Git remote for sync";
    };
    gitBranch = lib.mkOption {
      type = lib.types.str;
      default = "main";
      description = "Git branch";
    };
    deviceName = lib.mkOption {
      type = lib.types.str;
      default = config.networking.hostName;
      description = "Device identifier";
    };
  };
  
  config = lib.mkIf cfg.enable {
    # Tailscale
    services.tailscale = {
      enable = true;
      authKey = cfg.tailscaleAuthKey;
      acceptRoutes = true;
      advertiseRoutes = [ "10.0.0.0/8" "192.168.0.0/16" ];
    };
    
    # Git for federation repo
    programs.git = {
      enable = true;
      userName = "orchestrator";
      userEmail = "orchestrator@${cfg.deviceName}";
    };
    
    # Federation directory
    systemd.tmpfiles.rules = [
      "d /var/lib/orchestrator/federation 0755 orchestrator orchestrator -"
      "d /etc/orchestrator/federation 0755 orchestrator orchestrator -"
    ];
    
    # Initialize federation git repo
    systemd.services.init-federation-repo = {
      description = "Initialize federation git repository";
      wantedBy = [ "multi-user.target" ];
      before = [ "orchestrator.service" ];
      conditionPathExists = "!/etc/orchestrator/federation/.git";
      serviceConfig = {
        Type = "oneshot";
        ExecStart = ''
          mkdir -p /etc/orchestrator/federation
          cd /etc/orchestrator/federation
          git init
          git config user.name "orchestrator"
          git config user.email "orchestrator@${cfg.deviceName}"
          mkdir -p proposals
          echo "# Federation" > README.md
          git add .
          git commit -m "Initial commit"
          ${lib.optionalString (cfg.gitRemote != null) ''
            git remote add origin ${cfg.gitRemote}
            git push -u origin ${cfg.gitBranch}
          ''}
        '';
        RemainAfterExit = true;
      };
    };
    
    # Sync service
    systemd.services.federation-sync = {
      description = "Federation sync";
      wantedBy = [ "multi-user.target" ];
      after = [ "tailscaled.service" "network-online.target" "init-federation-repo.service" ];
      serviceConfig = {
        Type = "simple";
        ExecStart = "${pkgs.bash}/bin/bash -c 'while true; do sleep ${toString cfg.syncInterval}; /run/current-system/sw/bin/orchestrator-federate sync; done'";
        Restart = "on-failure";
        RestartSec = 30;
        User = "orchestrator";
      };
    };
    
    # Environment
    systemd.services.orchestrator.environment = {
      ORCHESTRATOR_FEDERATION_ENABLED = "true";
      ORCHESTRATOR_AUTOMERGE_PATH = cfg.automergePath;
      ORCHESTRATOR_GIT_REMOTE = cfg.gitRemote or "";
      ORCHESTRATOR_GIT_BRANCH = cfg.gitBranch;
      ORCHESTRATOR_SYNC_INTERVAL = toString cfg.syncInterval;
      ORCHESTRATOR_DEVICE_NAME = cfg.deviceName;
    };
  };
}