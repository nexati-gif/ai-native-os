//! eBPF program definitions and utilities

/// Scheduler eBPF program
pub const SCHEDULER_PROGRAM: &str = include_str!("../ebpf_programs/scheduler.bpf.c");

/// Network eBPF program
pub const NETWORK_PROGRAM: &str = include_str!("../ebpf_programs/network.bpf.c");

/// Block I/O eBPF program
pub const BLOCK_PROGRAM: &str = include_str!("../ebpf_programs/block.bpf.c");

/// Memory eBPF program
pub const MEMORY_PROGRAM: &str = include_str!("../ebpf_programs/memory.bpf.c");

/// Get all program names
pub fn program_names() -> &'static [&'static str] {
    &["scheduler", "network", "block", "memory"]
}

/// Get program source by name
pub fn get_program_source(name: &str) -> Option<&'static str> {
    match name {
        "scheduler" => Some(SCHEDULER_PROGRAM),
        "network" => Some(NETWORK_PROGRAM),
        "block" => Some(BLOCK_PROGRAM),
        "memory" => Some(MEMORY_PROGRAM),
        _ => None,
    }
}