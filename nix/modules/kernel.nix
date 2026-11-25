{ config, pkgs, lib, ... }:
let
  cfg = config.orchestrator.kernel;
in {
  options.orchestrator.kernel = {
    enable = lib.mkEnableOption "Custom kernel for AI-Native OS";
    version = lib.mkOption {
      type = lib.types.str;
      default = "6.6";
      description = "Kernel version (LTS)";
    };
    extraConfig = lib.mkOption {
      type = lib.types.lines;
      default = ""
        ''
        # sched_ext for custom schedulers
        CONFIG_SCHED_EXT=y
        CONFIG_SCHED_EXT_DEBUG=y
        
        # BPF LSM for security policies
        CONFIG_BPF_LSM=y
        CONFIG_BPF=y
        CONFIG_BPF_SYSCALL=y
        CONFIG_BPF_JIT=y
        CONFIG_BPF_JIT_ALWAYS_ON=y
        CONFIG_BPF_EVENTS=y
        CONFIG_BPF_KPROBE_OVERRIDE=y
        
        # Tracing
        CONFIG_FTRACE=y
        CONFIG_FUNCTION_TRACER=y
        CONFIG_FUNCTION_GRAPH_TRACER=y
        CONFIG_SCHED_TRACER=y
        CONFIG_HIST_TRIGGERS=y
        CONFIG_RING_BUFFER=y
        CONFIG_EVENT_TRACING=y
        CONFIG_CONTEXT_SWITCH_TRACER=y
        CONFIG_RING_BUFFER_BENCHMARK=m
        
        # Perf events
        CONFIG_PERF_EVENTS=y
        CONFIG_PERF_EVENTS_INTEL_UNCORE=y
        CONFIG_PERF_EVENTS_INTEL_RAPL=y
        CONFIG_PERF_EVENTS_INTEL_CSTATE=y
        CONFIG_PERF_EVENTS_AMD_POWER=y
        
        # eBPF map types
        CONFIG_BPF_MAP_TYPE_ARRAY=y
        CONFIG_BPF_MAP_TYPE_HASH=y
        CONFIG_BPF_MAP_TYPE_PROG_ARRAY=y
        CONFIG_BPF_MAP_TYPE_PERF_EVENT_ARRAY=y
        CONFIG_BPF_MAP_TYPE_RINGBUF=y
        CONFIG_BPF_MAP_TYPE_TASK_STORAGE=y
        CONFIG_BPF_MAP_TYPE_SK_STORAGE=y
        CONFIG_BPF_MAP_TYPE_DEVMAP=y
        CONFIG_BPF_MAP_TYPE_SOCKMAP=y
        CONFIG_BPF_MAP_TYPE_CPUMAP=y
        CONFIG_BPF_MAP_TYPE_XSKMAP=y
        CONFIG_BPF_MAP_TYPE_SOCKHASH=y
        CONFIG_BPF_MAP_TYPE_CGRP_STORAGE=y
        CONFIG_BPF_MAP_TYPE_REUSEPORT_SOCKARRAY=y
        CONFIG_BPF_MAP_TYPE_PERCPU_HASH=y
        CONFIG_BPF_MAP_TYPE_LRU_HASH=y
        CONFIG_BPF_MAP_TYPE_LRU_PERCPU_HASH=y
        CONFIG_BPF_MAP_TYPE_BLOOM_FILTER=y
        CONFIG_BPF_MAP_TYPE_USER_RINGBUF=y
        CONFIG_BPF_MAP_TYPE_CGRP_SOCK_ADDR=y
        CONFIG_BPF_MAP_TYPE_CGRP_SOCK_OPT=y
        
        # Kernel modules
        CONFIG_MODULES=y
        CONFIG_MODULE_UNLOAD=y
        CONFIG_MODULE_FORCE_UNLOAD=y
        CONFIG_MODULE_SIG=y
        CONFIG_MODULE_SIG_FORCE=y
        CONFIG_MODULE_SIG_ALL=y
        CONFIG_MODULE_SIG_SHA512=y
        CONFIG_MODULE_COMPRESS_ZSTD=y
        
        # Memory management
        CONFIG_TRANSPARENT_HUGEPAGE=y
        CONFIG_TRANSPARENT_HUGEPAGE_ALWAYS=y
        CONFIG_ZSWAP=y
        CONFIG_ZSWAP_COMPRESSOR_DEFAULT="zstd"
        CONFIG_ZSWAP_ZPOOL_DEFAULT="z3fold"
        
        # Scheduler
        CONFIG_SCHED_DEBUG=y
        CONFIG_SCHED_INFO=y
        CONFIG_SCHEDSTATS=y
        CONFIG_SCHED_SMT=y
        CONFIG_SCHED_MC=y
        CONFIG_SCHED_AUTOGROUP=y
        CONFIG_FAIR_GROUP_SCHED=y
        CONFIG_CFS_BANDWIDTH=y
        CONFIG_RT_GROUP_SCHED=y
        CONFIG_UCLAMP_TASK=y
        CONFIG_UCLAMP_BUCKETS_COUNT=20
        
        # CPU frequency
        CONFIG_CPU_FREQ=y
        CONFIG_CPU_FREQ_GOV_ATTR_SET=y
        CONFIG_CPU_FREQ_GOV_COMMON=y
        CONFIG_CPU_FREQ_GOV_PERFORMANCE=y
        CONFIG_CPU_FREQ_GOV_POWERSAVE=y
        CONFIG_CPU_FREQ_GOV_USERSPACE=y
        CONFIG_CPU_FREQ_GOV_ONDEMAND=y
        CONFIG_CPU_FREQ_GOV_CONSERVATIVE=y
        CONFIG_CPU_FREQ_GOV_SCHEDUTIL=y
        
        # Power management
        CONFIG_PM=y
        CONFIG_PM_SLEEP=y
        CONFIG_PM_AUTOSLEEP=y
        CONFIG_PM_WAKELOCKS=y
        CONFIG_PM_WAKELOCKS_LIMIT=100
        CONFIG_PM_WAKELOCKS_GC=y
        CONFIG_SUSPEND=y
        CONFIG_HIBERNATION=y
        CONFIG_ACPI=y
        CONFIG_ACPI_SLEEP=y
        CONFIG_ACPI_PROCFS_POWER=y
        
        # Security
        CONFIG_SECURITY=y
        CONFIG_SECURITYFS=y
        CONFIG_SECURITY_NETWORK=y
        CONFIG_SECURITY_INFINIBAND=y
        CONFIG_SECURITY_NETWORK_XFRM=y
        CONFIG_SECURITY_PATH=y
        CONFIG_INTEL_TXT=y
        CONFIG_HAVE_HARDENED_USERCOPY_ALLOCATOR=y
        CONFIG_HARDENED_USERCOPY=y
        CONFIG_HARDENED_USERCOPY_FALLBACK=y
        CONFIG_FORTIFY_SOURCE=y
        CONFIG_STATIC_USERMODEHELPER=y
        CONFIG_STATIC_USERMODEHELPER_PATH="/usr/lib/systemd/systemd"
        
        # Lockdown
        CONFIG_SECURITY_LOCKDOWN_LSM=y
        CONFIG_SECURITY_LOCKDOWN_LSM_EARLY=y
        
        # Random
        CONFIG_RANDOM_TRUST_CPU=y
        ''
      ;
      patches = lib.mkOption {
        type = lib.types.listOf lib.types.path;
        default = [ ];
        description = "Kernel patches to apply";
      };
      modules = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        description = "Kernel modules to build";
      };
      cmdline = lib.mkOption {
        type = lib.types.str;
        default = "init_on_alloc=1 init_on_free=1 page_poison=1 slub_debug=FZP module.sig_enforce=1 lockdown=confidentiality";
        description = "Kernel command line";
      };
  };
  
  config = lib.mkIf cfg.enable {
    boot.kernelPackages = pkgs.linuxPackages_${cfg.version};
    boot.kernelPackages.kernel.config = cfg.extraConfig;
    boot.kernelPackages.kernel.patches = cfg.patches;
    boot.kernelPackages.kernel.extraModulePackages = cfg.modules;
    boot.kernelParams = lib.splitString " " cfg.cmdline;
    
    # Build custom kernel modules
    boot.kernelModules = cfg.modules;
    
    # Enable kernel module signing
    boot.kernelModules = lib.mkForce cfg.modules;
  };
}