{ config, pkgs, lib, ... }:
let
  cfg = config.orchestrator.sandbox;
in {
  options.orchestrator.sandbox = {
    enable = lib.mkEnableOption "Sandbox for proposal testing";
    backend = lib.mkOption {
      type = lib.types.enum [ "firecracker" "qemu" "both" ];
      default = "firecracker";
      description = "Sandbox backend";
    };
    firecracker = {
      vcpus = lib.mkOption { type = lib.types.int; default = 2; };
      memoryMiB = lib.mkOption { type = lib.types.int; default = 512; };
      kernelPath = lib.mkOption { type = lib.types.path; default = "${config.boot.kernelPackages.kernel}/Image"; };
      initrdPath = lib.mkOption { type = lib.types.path; default = "${config.boot.initrd}/initrd"; };
      rootfsPath = lib.mkOption { type = lib.types.path; default = "/var/lib/orchestrator/rootfs.ext4"; };
      bootArgs = lib.mkOption { type = lib.types.str; default = "console=ttyS0 reboot=k panic=1 pci=off nomodules"; };
    };
    qemu = {
      vcpus = lib.mkOption { type = lib.types.int; default = 2; };
      memoryMiB = lib.mkOption { type = lib.types.int; default = 1024; };
      kernelPath = lib.mkOption { type = lib.types.path; default = "${config.boot.kernelPackages.kernel}/Image"; };
      initrdPath = lib.mkOption { type = lib.types.path; default = "${config.boot.initrd}/initrd"; };
      diskPath = lib.mkOption { type = lib.types.path; default = "/var/lib/orchestrator/disk.qcow2"; };
      enableKvm = lib.mkOption { type = lib.types.bool; default = true; };
    };
    warmPoolSize = lib.mkOption {
      type = lib.types.int;
      default = 0;
      description = "Number of pre-warmed microVMs (0 = on-demand only)";
    };
    testRootfs = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/orchestrator/test-rootfs";
      description = "Minimal test rootfs";
    };
  };
  
  config = lib.mkIf cfg.enable {
    # Firecracker
    virtualisation.firecracker.enable = lib.mkForce (cfg.backend == "firecracker" || cfg.backend == "both");
    
    # QEMU
    virtualisation.qemu = {
      enable = lib.mkForce (cfg.backend == "qemu" || cfg.backend == "both");
      package = pkgs.qemu_kvm;
    };
    
    # KVM
    virtualisation.kvm.enable = lib.mkForce (cfg.backend == "qemu" || cfg.backend == "both");
    
    # Create test rootfs
    systemd.services.create-test-rootfs = {
      description = "Create minimal test rootfs";
      wantedBy = [ "multi-user.target" ];
      before = [ "orchestrator.service" ];
      serviceConfig = {
        Type = "oneshot";
        ExecStart = ''
          mkdir -p ${cfg.testRootfs}
          # Create minimal init
          cat > ${cfg.testRootfs}/init <<'EOF'
          #!/bin/sh
          mount -t proc proc /proc
          mount -t sysfs sysfs /sys
          mount -t devtmpfs devtmpfs /dev
          exec /bin/sh
          EOF
          chmod +x ${cfg.testRootfs}/init
          
          # Copy essential binaries
          for bin in sh ls cat echo test mkdir mount umount; do
            cp $(which $bin) ${cfg.testRootfs}/ 2>/dev/null || true
          done
          
          # Create rootfs.ext4 for firecracker
          if [ ! -f ${cfg.firecracker.rootfsPath} ]; then
            dd if=/dev/zero of=${cfg.firecracker.rootfsPath} bs=1M count=2048
            mkfs.ext4 -F ${cfg.firecracker.rootfsPath}
            mkdir -p /mnt/rootfs
            mount ${cfg.firecracker.rootfsPath} /mnt/rootfs
            cp -r ${cfg.testRootfs}/* /mnt/rootfs/
            umount /mnt/rootfs
          fi
          
          # Create disk.qcow2 for QEMU
          if [ ! -f ${cfg.qemu.diskPath} ]; then
            qemu-img create -f qcow2 ${cfg.qemu.diskPath} 4G
            # Would need to install NixOS to it
          fi
        '';
        RemainAfterExit = true;
      };
    };
    
    # Warm pool (optional)
    systemd.services.warm-pool = lib.mkIf (cfg.warmPoolSize > 0) {
      description = "Warm sandbox pool";
      wantedBy = [ "multi-user.target" ];
      after = [ "create-test-rootfs.service" ];
      serviceConfig = {
        Type = "oneshot";
        ExecStart = ''
          for i in $(seq 1 ${toString cfg.warmPoolSize}); do
            # Start firecracker microVMs and keep them paused
            firecracker --api-sock /tmp/fc-warm-$i.sock &
          done
        '';
        RemainAfterExit = true;
      };
    };
  };
}