{ config, pkgs, lib, ... }:
let
  cfg = config.orchestrator;
in {
  options.orchestrator = {
    enable = lib.mkEnableOption "AI-Native OS Orchestrator";
    configFile = lib.mkOption {
      type = lib.types.path;
      default = "/etc/orchestrator/config.toml";
      description = "Orchestrator config file";
    };
    deviceId = lib.mkOption {
      type = lib.types.str;
      default = config.networking.hostName;
      description = "Unique device identifier";
    };
    stateDbPath = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/orchestrator/state.redb";
      description = "State database path";
    };
    idleTimeout = lib.mkOption {
      type = lib.types.int;
      default = 30;
      description = "Idle timeout for model unload (seconds)";
    };
    passive = {
      enable = lib.mkOption { type = lib.types.bool; default = true; };
      pollIntervalMs = lib.mkOption { type = lib.types.int; default = 100; };
      eventsBufferSize = lib.mkOption { type = lib.types.int; default = 10000; };
      persistIntervalSecs = lib.mkOption { type = lib.types.int; default = 60; };
      collectors = lib.mkOption {
        type = lib.types.listOf (lib.types.submodule {
          options = {
            name = lib.mkOption { type = lib.types.str; };
            enabled = lib.mkOption { type = lib.types.bool; default = true; };
            config = lib.mkOption { type = lib.types.json; default = { }; };
          };
        });
        default = [
          { name = "scheduler"; enabled = true; }
          { name = "network"; enabled = true; }
          { name = "block"; enabled = true; }
          { name = "memory"; enabled = true; }
        ];
      };
    };
    active = {
      maxConcurrentCycles = lib.mkOption { type = lib.types.int; default = 1; };
      proposalTimeoutSecs = lib.mkOption { type = lib.types.int; default = 300; };
      sandboxTimeoutSecs = lib.mkOption { type = lib.types.int; default = 600; };
      validationTimeoutSecs = lib.mkOption { type = lib.types.int; default = 300; };
      autoPromoteOnSuccess = lib.mkOption { type = lib.types.bool; default = false; };
    };
    ui = {
      mode = lib.mkOption {
        type = lib.types.enum [ "tui" "web" "both" ];
        default = "both";
      };
      tuiEnabled = lib.mkOption { type = lib.types.bool; default = true; };
      webEnabled = lib.mkOption { type = lib.types.bool; default = true; };
      webPort = lib.mkOption { type = lib.types.int; default = 8080; };
      webBind = lib.mkOption { type = lib.types.str; default = "127.0.0.1"; };
      authToken = lib.mkOption { type = lib.types.nullOr lib.types.str; default = null; };
    };
    policy = {
      engine = lib.mkOption {
        type = lib.types.enum [ "opa" "custom" ];
        default = "opa";
      };
      policiesDir = lib.mkOption { type = lib.types.path; default = "/etc/orchestrator/policies"; };
      defaultDeny = lib.mkOption { type = lib.types.bool; default = true; };
    };
    kernel = lib.mkOption { type = lib.types.submodule { options = import ./kernel.nix; }; };
    inference = lib.mkOption { type = lib.types.submodule { options = import ./inference.nix; }; };
    sandbox = lib.mkOption { type = lib.types.submodule { options = import ./sandbox.nix; }; };
    federation = lib.mkOption { type = lib.types.submodule { options = import ./federation.nix; }; };
  };
  
  config = lib.mkIf cfg.enable {
    # User and group
    users.users.orchestrator = {
      isSystemUser = true;
      group = "orchestrator";
      home = "/var/lib/orchestrator";
      shell = pkgs.bash;
    };
    users.groups.orchestrator = { };
    
    # Directories
    systemd.tmpfiles.rules = [
      "d /var/lib/orchestrator 0755 orchestrator orchestrator -"
      "d /var/lib/orchestrator/lora 0755 orchestrator orchestrator -"
      "d /var/lib/orchestrator/rag 0755 orchestrator orchestrator -"
      "d /etc/orchestrator 0755 root root -"
      "d /etc/orchestrator/policies 0755 root root -"
      "f /etc/orchestrator/config.toml 0640 root orchestrator -"
    ];
    
    # Generate config file
    systemd.services.orchestrator-config = {
      description = "Generate orchestrator config";
      wantedBy = [ "multi-user.target" ];
      before = [ "orchestrator.service" ];
      serviceConfig = {
        Type = "oneshot";
        ExecStart = ''
          mkdir -p /etc/orchestrator
          cat > /etc/orchestrator/config.toml <<'EOF'
device_id = "${cfg.deviceId}"
model_path = "${config.orchestrator.inference.modelPath}"
rag_path = "${config.orchestrator.inference.ragPath}"
lora_path = "${config.orchestrator.inference.loraPath}"
state_db_path = "${cfg.stateDbPath}"
idle_timeout_secs = ${toString cfg.idleTimeout}
passive_mode = { enabled = ${toString cfg.passive.enable}, poll_interval_ms = ${toString cfg.passive.pollIntervalMs}, events_buffer_size = ${toString cfg.passive.eventsBufferSize}, persist_interval_secs = ${toString cfg.passive.persistIntervalSecs}, collectors = [] }
active_mode = { max_concurrent_cycles = ${toString cfg.active.maxConcurrentCycles}, proposal_timeout_secs = ${toString cfg.active.proposalTimeoutSecs}, sandbox_timeout_secs = ${toString cfg.active.sandboxTimeoutSecs}, validation_timeout_secs = ${toString cfg.active.validationTimeoutSecs}, auto_promote_on_success = ${toString cfg.active.autoPromoteOnSuccess} }
federation = { enabled = ${toString config.orchestrator.federation.enable}, tailscale_auth_key = null, sync_interval_secs = ${toString config.orchestrator.federation.syncInterval}, automerge_doc_path = "${config.orchestrator.federation.automergePath}", git_remote = ${lib.optionalString (config.orchestrator.federation.gitRemote != null) ("\"" + config.orchestrator.federation.gitRemote + "\"")} "null", git_branch = "${config.orchestrator.federation.gitBranch}" }
ui = { mode = "${cfg.ui.mode}", tui_enabled = ${toString cfg.ui.tuiEnabled}, web_enabled = ${toString cfg.ui.webEnabled}, web_port = ${toString cfg.ui.webPort}, web_bind = "${cfg.ui.webBind}", auth_token = ${lib.optionalString (cfg.ui.authToken != null) ("\"" + cfg.ui.authToken + "\"")} "null" }
policy = { engine = "${cfg.policy.engine}", policies_dir = "${cfg.policy.policiesDir}", default_deny = ${toString cfg.policy.defaultDeny} }
sandbox = { backend = "${config.orchestrator.sandbox.backend}", firecracker = { vcpus = ${toString config.orchestrator.sandbox.firecracker.vcpus}, memory_mib = ${toString config.orchestrator.sandbox.firecracker.memoryMiB}, kernel_path = "${config.orchestrator.sandbox.firecracker.kernelPath}", initrd_path = "${config.orchestrator.sandbox.firecracker.initrdPath}", rootfs_path = "${config.orchestrator.sandbox.firecracker.rootfsPath}", boot_args = "${config.orchestrator.sandbox.firecracker.bootArgs}" }, qemu = { vcpus = ${toString config.orchestrator.sandbox.qemu.vcpus}, memory_mib = ${toString config.orchestrator.sandbox.qemu.memoryMiB}, kernel_path = "${config.orchestrator.sandbox.qemu.kernelPath}", initrd_path = "${config.orchestrator.sandbox.qemu.initrdPath}", disk_path = "${config.orchestrator.sandbox.qemu.diskPath}", enable_kvm = ${toString config.orchestrator.sandbox.qemu.enableKvm} }, warm_pool_size = ${toString config.orchestrator.sandbox.warmPoolSize} }
EOF
        '';
        RemainAfterExit = true;
      };
    };
    
    # Orchestrator service
    systemd.services.orchestrator = {
      description = "AI-Native OS Orchestrator";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" "tailscaled.service" "orchestrator-config.service" ];
      requires = [ "orchestrator-config.service" ];
      serviceConfig = {
        Type = "notify";
        ExecStart = "${pkgs.orchestrator}/bin/orchestrator --config /etc/orchestrator/config.toml";
        Restart = "on-failure";
        RestartSec = 5;
        # Memory limits (passive mode only uses ~50MB)
        MemoryLimit = "512M";
        MemorySwapMax = "0";
        CPUQuota = "200%";
        # Security
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        NoNewPrivileges = true;
        CapabilityBoundingSet = "CAP_SYS_ADMIN CAP_SYS_RESOURCE CAP_BPF CAP_DAC_OVERRIDE CAP_SYS_MODULE";
        AmbientCapabilities = "CAP_SYS_ADMIN CAP_SYS_RESOURCE CAP_BPF CAP_DAC_OVERRIDE CAP_SYS_MODULE";
        # Read-only paths
        ReadOnlyPaths = "/etc /usr /boot";
        ReadWritePaths = "/var/lib/orchestrator /etc/orchestrator /run /tmp";
      };
      environment = {
        RUST_LOG = "orchestrator=info,orchestrator_observe=debug,orchestrator_plan=debug";
        RUST_BACKTRACE = "1";
      };
    };
    
    # UKI (Unified Kernel Image) for atomic boot
    boot.loader.systemd-boot.enable = true;
    boot.loader.efi.canTouchEfiVariables = true;
    boot.loader.efi.efiSysMountPoint = "/boot/efi";
    
    # systemd-sysupdate for atomic upgrades
    systemd.sysupdate = {
      enable = true;
      unit = "orchestrator.service";
    };
    
    # Default policies
    systemd.services.install-default-policies = {
      description = "Install default OPA policies";
      wantedBy = [ "multi-user.target" ];
      before = [ "orchestrator.service" ];
      serviceConfig = {
        Type = "oneshot";
        ExecStart = ''
          mkdir -p /etc/orchestrator/policies
          cat > /etc/orchestrator/policies/base.rego <<'EOF'
package orchestrator

default allow = false
default reason = "No matching policy"
default required_approvals = 1
default max_risk_level = "critical"

allow {
    input.proposal.kind == "SysctlTuning"
    input.proposal.risk_level == "Low"
    input.proposal.estimated_impact.stability_risk == "Low"
    input.proposal.estimated_impact.security_risk == "None"
}

allow {
    input.proposal.kind == "EbpfProgram"
    input.proposal.risk_level == "Medium"
    count(input.proposal.test_plan.integration_tests) > 0
    input.proposal.test_plan.canary_duration_secs >= 300
}

allow {
    input.proposal.kind == "NixConfig"
    input.proposal.rollback_plan.automatic == true
    input.proposal.test_plan.canary_duration_secs >= 180
}

allow {
    input.proposal.kind == "KernelPatch"
    input.proposal.risk_level == "Critical"
    count(input.proposal.test_plan.integration_tests) >= 3
    count(input.proposal.test_plan.benchmarks) >= 2
    input.proposal.test_plan.canary_duration_secs >= 3600
    input.proposal.test_plan.success_criteria.statistical_significance <= 0.01
}

required_approvals = 2 {
    input.proposal.risk_level == "High"
}

required_approvals = 3 {
    input.proposal.risk_level == "Critical"
}

deny_unsafe {
    input.proposal.rollback_plan.data_loss_risk != "None"
}

deny_unsafe {
    not input.proposal.rollback_plan.automatic
    input.proposal.risk_level != "Low"
}

deny_unsafe {
    input.proposal.test_plan.canary_duration_secs == 0
    input.proposal.risk_level != "Low"
}

allow = false {
    deny_unsafe
    reason = "Safety violation: " + deny_reason
}

deny_reason = "Data loss risk" {
    input.proposal.rollback_plan.data_loss_risk != "None"
}

deny_reason = "No automatic rollback" {
    not input.proposal.rollback_plan.automatic
    input.proposal.risk_level != "Low"
}

deny_reason = "No canary period" {
    input.proposal.test_plan.canary_duration_secs == 0
    input.proposal.risk_level != "Low"
}
EOF
        '';
        RemainAfterExit = true;
      };
    };
  };
}