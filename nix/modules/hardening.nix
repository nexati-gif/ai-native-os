{ config, pkgs, lib, ... }:
{
  options.orchestrator.hardening = {
    enable = lib.mkEnableOption "Security hardening";
    kernelLockdown = lib.mkOption {
      type = lib.types.enum [ "none" "integrity" "confidentiality" ];
      default = "confidentiality";
    };
    moduleSigEnforce = lib.mkOption { type = lib.types.bool; default = true; };
    restrictSyscalls = lib.mkOption { type = lib.types.bool; default = true; };
    disableUnusedFilesystems = lib.mkOption { type = lib.types.bool; default = true; };
    disableUnusedNetwork = lib.mkOption { type = lib.types.bool; default = true; };
    hideKernelSymbols = lib.mkOption { type = lib.types.bool; default = true; };
    restrictPerf = lib.mkOption { type = lib.types.bool; default = true; };
    restrictDmesg = lib.mkOption { type = lib.types.bool; default = true; };
    restrictKptrRestrict = lib.mkOption { type = lib.types.bool; default = true; };
  };
  
  config = lib.mkIf config.orchestrator.hardening.enable {
    # Kernel lockdown mode
    boot.kernelParams = lib.mkForce [
      "kernel.lockdown=${config.orchestrator.hardening.kernelLockdown}"
      "module.sig_enforce=${lib.boolToInt config.orchestrator.hardening.moduleSigEnforce}"
    ];
    
    # Sysctl hardening
    boot.kernel.sysctl = {
      # Restrict dmesg
      "kernel.dmesg_restrict" = lib.mkForce (lib.boolToInt config.orchestrator.hardening.restrictDmesg);
      # Restrict perf
      "kernel.perf_event_paranoid" = lib.mkForce (if config.orchestrator.hardening.restrictPerf then 3 else 1);
      # Restrict kptr
      "kernel.kptr_restrict" = lib.mkForce (lib.boolToInt config.orchestrator.hardening.restrictKptrRestrict);
      # Hide kernel symbols
      "kernel.kallsyms" = lib.mkForce (if config.orchestrator.hardening.hideKernelSymbols then 0 else 1);
      # SysRq
      "kernel.sysrq" = lib.mkForce 0;
      # Panic on oops
      "kernel.panic_on_oops" = lib.mkForce 1;
      "kernel.panic" = lib.mkForce 10;
      # Restrict unprivileged bpf
      "kernel.unprivileged_bpf_disabled" = lib.mkForce 1;
      # Restrict user namespaces
      "kernel.unprivileged_userns_clone" = lib.mkForce 0;
      # ASLR
      "kernel.randomize_va_space" = lib.mkForce 2;
      # Hardlinks/symlinks
      "fs.protected_hardlinks" = lib.mkForce 1;
      "fs.protected_symlinks" = lib.mkForce 1;
      # FIFO/regular files
      "fs.protected_fifos" = lib.mkForce 2;
      "fs.protected_regular" = lib.mkForce 2;
    };
    
    # Disable unused filesystems
    boot.kernelModules = lib.mkIf config.orchestrator.hardening.disableUnusedFilesystems [
      # Only load needed filesystems
    ];
    
    boot.blacklistedKernelModules = lib.mkIf config.orchestrator.hardening.disableUnusedFilesystems [
      "cramfs" "freevxfs" "jffs2" "hfs" "hfsplus" "squashfs" "udf" "affs" "efs"
      "cifs" "nfs" "nfsd" "smbfs" "9p" "fuse" "ceph" "gfs2" "ocfs2"
    ];
    
    # Disable unused network protocols
    boot.blacklistedKernelModules = lib.mkIf config.orchestrator.hardening.disableUnusedNetwork [
      "dccp" "sctp" "rds" "tipc" "atm" "ax25" "netrom" "rose" "x25"
      "bluetooth" "rfcomm" "bnep" "hidp" "6lowpan" "ieee802154"
    ];
    
    # systemd hardening
    systemd = {
      # Mask unused services
      maskedServices = [
        "systemd-timesyncd" "systemd-resolved" "systemd-networkd"
        "ModemManager" "bluetooth" "cups" "avahi-daemon"
        "colord" "geoclue" "fwupd" "bolt" "upower"
        "rtkit-daemon" "polkit" "accounts-daemon"
        "udisks2" "gvfs" "tracker" "packagekit"
      ];
      
      # Restrict default units
      serviceDefaults = {
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        NoNewPrivileges = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        RemoveIPC = true;
        SystemCallFilter = "@system-service";
        SystemCallErrorNumber = "EPERM";
      };
    };
    
    # Audit
    security.audit = {
      enable = true;
      rules = [
        "-w /etc/passwd -p wa -k identity"
        "-w /etc/group -p wa -k identity"
        "-w /etc/shadow -p wa -k identity"
        "-w /etc/sudoers -p wa -k sudo"
        "-w /etc/orchestrator -p wa -k orchestrator"
        "-a always,exit -F arch=b64 -S execve -k exec"
        "-a always,exit -F arch=b64 -S open,openat -F exit=-EACCES -k access"
        "-a always,exit -F arch=b64 -S mount,umount2 -k mount"
        "-a always,exit -F arch=b64 -S reboot -k reboot"
      ];
    };
    
    # AppArmor (if preferred over SELinux)
    security.apparmor = {
      enable = true;
      profiles = {
        "orchestrator" = ''
          #include <tunables/global>
          profile orchestrator /run/current-system/sw/bin/orchestrator flags=(attach_disconnected) {
            #include <abstractions/base>
            capability sys_admin,
            capability sys_resource,
            capability bpf,
            capability dac_override,
            capability sys_module,
            network,
            /etc/orchestrator/** r,
            /var/lib/orchestrator/** rw,
            /run/current-system/sw/bin/orchestrator r,
            /nix/store/** r,
          }
        '';
      };
    };
    
    # SSH hardening
    services.openssh = {
      enable = false;  # Disable by default, enable only if needed
      settings = {
        PermitRootLogin = "no";
        PasswordAuthentication = "no";
        PubkeyAuthentication = "yes";
        PermitEmptyPasswords = "no";
        ChallengeResponseAuthentication = "no";
        UsePAM = "yes";
        X11Forwarding = "no";
        AllowAgentForwarding = "no";
        AllowTcpForwarding = "no";
        PermitTunnel = "no";
        ClientAliveInterval = "300";
        ClientAliveCountMax = "2";
        MaxAuthTries = "3";
        MaxSessions = "2";
      };
    };
    
    # Firewall
    networking.firewall = {
      enable = true;
      allowedTCPPorts = [ ];
      allowedUDPPorts = [ ];
      # Only allow Tailscale
      checkReversePath = true;
      logReversePathDrops = true;
      logInvalidPackets = true;
    };
  };
}