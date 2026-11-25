// Build script for eBPF programs

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    
    // Only build eBPF programs on Linux
    if env::var("CARGO_CFG_TARGET_OS").unwrap() != "linux" {
        println!("cargo:warning=Not on Linux, skipping eBPF build");
        return;
    }
    
    // Check for clang
    let clang = env::var("CLANG").unwrap_or_else(|_| "clang".to_string());
    let clang_version = Command::new(&clang)
        .arg("--version")
        .output()
        .expect("Failed to run clang");
    
    if !clang_version.status.success() {
        println!("cargo:warning=clang not found, skipping eBPF build");
        return;
    }
    
    // Build each eBPF program
    let programs = [
        ("scheduler", "src/ebpf_programs/scheduler.bpf.c"),
        ("network", "src/ebpf_programs/network.bpf.c"),
        ("block", "src/ebpf_programs/block.bpf.c"),
        ("memory", "src/ebpf_programs/memory.bpf.c"),
    ];
    
    for (name, source) in &programs {
        let output = PathBuf::from(&out_dir).join(format!("{}.bpf.o", name));
        let source_path = PathBuf::from(source);
        
        if !source_path.exists() {
            println!("cargo:warning=eBPF source not found: {}", source);
            continue;
        }
        
        let status = Command::new(&clang)
            .args([
                "-target", "bpf",
                "-O2",
                "-g",
                "-D__BPF_TRACING__",
                "-D__KERNEL__",
                "-Wno-unused-value",
                "-Wno-pointer-sign",
                "-Wno-compare-distinct-pointer-types",
                "-Wno-gnu-variable-sized-type-not-at-end",
                "-Wno-address-of-packed-member",
                "-Wno-tautological-compare",
                "-Wno-unknown-warning-option",
                "-I", "/usr/include",
                "-I", "/usr/include/linux",
                "-c", source,
                "-o", output.to_str().unwrap(),
            ])
            .status()
            .expect("Failed to execute clang");
        
        if !status.success() {
            println!("cargo:warning=Failed to compile eBPF program: {}", name);
            continue;
        }
        
        println!("cargo:rustc-env={}_EBPF_PATH={}", name.to_uppercase(), output.display());
        println!("cargo:rerun-if-changed={}", source);
    }
    
    // Also check for bpftool for skeleton generation
    let bpftool = env::var("BPFTOOL").unwrap_or_else(|_| "bpftool".to_string());
    let _ = Command::new(&bpftool).arg("--version").output();
}

fn target_arch_to_bpf(arch: &str) -> &'static str {
    match arch {
        "x86_64" => "x86",
        "aarch64" => "arm64",
        _ => "unknown",
    }
}