# AI-Native OS Orchestrator

> A self-modifying, federated operating system where an AI agent continuously observes, plans, and optimizes the system — with human approval for every change.

## Vision

An operating system that **learns from your usage patterns**, **proposes optimizations**, and **implements them safely** — all while you remain in control. The orchestrator runs as a persistent agent that:

1. **Observes** system metrics (scheduler, memory, network, block) via eBPF
2. **Plans** optimizations using a local LLM (Qwen2.5-Coder-32B) + RAG
3. **Proposes** changes with risk assessment, test plans, and rollback strategies
4. **Waits for your approval** via TUI or Web UI
5. **Executes** in isolated Firecracker microVMs
6. **Validates** statistically before promoting
7. **Federates** knowledge across your devices via Tailscale + Automerge + Git

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         FEDERATION (Tailscale + Automerge + Git)            │
│  Laptop ◄──────────────────────────────────────────────────────► Phone      │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
        ┌───────────────────────────┼───────────────────────────┐
        ▼                           ▼                           ▼
┌───────────────┐           ┌───────────────┐           ┌───────────────┐
│  DEVICE A     │           │  DEVICE B     │           │  CLOUD GPU    │
│  (Laptop)     │           │  (Phone)      │           │  (Optional)   │
│               │           │               │           │               │
│ Qwen2.5-32B   │◄─────────►│ Qwen2.5-7B    │◄─────────►│ Qwen2.5-72B   │
│ 4-bit (16GB)  │  LoRA/RAG │ 3B (4-8GB)    │  LoRA/RAG │ or DeepSeek   │
│               │  Sync     │               │  Sync     │ -V2 (236B)    │
│ Policy Local: │           │ Policy Local: │           │               │
│ - Sysctl/eBPF │           │ - Configs     │           │ Reasoning:    │
│ - .ko builds  │           │ - UI tweaks   │           │ - Kernel core │
│ - Nix flakes  │           │ - Apps        │           │ - Architecture│
└───────────────┘           └───────────────┘           └───────────────┘
```

### Core Components

| Crate | Responsibility |
|-------|----------------|
| `orchestrator-core` | Main loop, state machine, lazy component loading |
| `orchestrator-observe` | eBPF collectors (scheduler, network, block, memory) |
| `orchestrator-plan` | LLM + RAG planner (candle/llama.cpp, Qdrant) |
| `orchestrator-act` | Nix builder, Firecracker/QEMU sandbox, kexec |
| `orchestrator-validate` | Statistical validation (t-test, bootstrap CI) |
| `orchestrator-federate` | Tailscale mesh, Automerge CRDT, Git sync |
| `orchestrator-ui` | TUI (ratatui) + Web (axum/HTMX) |
| `orchestrator-policy` | OPA/Rego policy engine |
| `orchestrator-common` | Shared types, errors, config, events, state |

## Quick Start

### Prerequisites

- NixOS 24.05+ with flakes enabled
- Hardware with virtualization support (KVM)
- 16GB+ RAM for 32B model (8GB for 7B model)
- GPU/NPU for accelerated inference (optional but recommended)

### Installation

```bash
# Clone the repository
git clone https://github.com/your-org/ai-native-os.git
cd ai-native-os

# Bootstrap (run as root on NixOS)
sudo ./scripts/bootstrap.sh

# Or manually:
# 1. Enter dev shell
nix develop

# 2. Build
cargo build --release --workspace

# 3. Deploy to NixOS
sudo nixos-rebuild switch --flake .#$(hostname)
```

### Configuration

1. Set your Tailscale auth key:
   ```bash
   export TAILSCALE_AUTH_KEY="tskey-xxx"
   ```

2. Configure model path in `/etc/orchestrator/config.toml`

3. Start the orchestrator:
   ```bash
   sudo systemctl start orchestrator
   ```

4. Access Web UI: http://localhost:8080

## Usage

### Triggering an Optimization Cycle

```bash
# Manual trigger via signal
sudo kill -USR1 $(pidof orchestrator)

# Or via Web UI: click "Run Optimization"
```

### Approving Proposals

1. **Web UI**: http://localhost:8080 — review diff, impact, test plan → Approve/Reject/Modify
2. **TUI**: Run `orchestrator-tui` in terminal — keyboard-driven approval
3. **API**: POST to `/proposal` with approval decision

### Monitoring

```bash
# Logs
journalctl -u orchestrator -f

# Metrics
curl http://localhost:8080/metrics

# State
cat /var/lib/orchestrator/state.redb  # (use redb CLI)
```

## Development

### Dev Shell

```bash
nix develop
# Or with direnv: direnv allow
```

Available commands:
- `cb` - cargo build
- `ct` - cargo test
- `ccl` - cargo clippy
- `cf` - cargo fmt
- `nr` - nixos-rebuild switch
- `or` - run orchestrator binary

### Running Tests

```bash
# Unit tests
cargo test --workspace

# Integration tests
./scripts/test-integration.sh

# Nextest (faster)
cargo nextest run --workspace
```

### Adding a New Proposal Type

1. Add variant to `ProposalKind` in `crates/common/src/types.rs`
2. Add policy rules in `crates/orchestrator-policy/src/engine.rs`
3. Implement Nix config application in `crates/orchestrator-act/src/nix.rs`
4. Add sandbox tests in `crates/orchestrator-act/src/sandbox.rs`
5. Update validation criteria in `crates/orchestrator-validate/src/validator.rs`

## Federation

Devices sync via:
- **Tailscale** — Mesh VPN for direct device-to-device communication
- **Automerge** — CRDT for conflict-free LoRA adapters, RAG chunks, policies
- **Git** — Immutable history of approved proposals and configurations

### Sync Flow

1. Proposal approved on Device A
2. LoRA adapter + RAG chunks + policy updates written to Automerge doc
3. Changes committed to Git
4. Tailscale notifies peers
5. Device B pulls Git + merges Automerge
6. Device B loads new LoRA adapter for improved planning

## Security

- **Kernel lockdown** mode (confidentiality)
- **Module signing** enforced
- **Systemd hardening** (PrivateTmp, ProtectSystem=strict, NoNewPrivileges, etc.)
- **AppArmor** profile for orchestrator
- **Audit** rules for critical paths
- **Firewall** — only Tailscale allowed by default
- **Policy engine** — default-deny, risk-based approvals
- **Sandbox** — Firecracker microVMs for all testing

## Hardware Support

| Device | Model | Inference | Kernel |
|--------|-------|-----------|--------|
| Laptop (x86_64) | Qwen2.5-Coder-32B-4bit | candle (CUDA/Metal) | 6.6 LTS + sched_ext |
| Phone (aarch64) | Qwen2.5-Coder-7B-4bit | candle (NPU/GPU) | 6.6 LTS + mobile patches |
| Server | Qwen2.5-Coder-32B-4bit | vLLM/TGI | 6.6 LTS |

## Roadmap

- [x] Core architecture & crate structure
- [x] NixOS modules (kernel, inference, sandbox, federation, hardening)
- [x] Policy engine (OPA/Rego)
- [x] TUI + Web UI skeletons
- [ ] eBPF collectors implementation
- [ ] Candle/llama.cpp model loading
- [ ] Qdrant RAG index
- [ ] Firecracker sandbox integration
- [ ] NixOS generation build/switch
- [ ] Statistical validation
- [ ] Automerge federation sync
- [ ] End-to-end integration tests
- [ ] Mobile (aarch64) support
- [ ] NPU acceleration (Qualcomm QNN)
- [ ] LoRA training pipeline

## License

Apache-2.0 — See [LICENSE](LICENSE)

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make changes with tests
4. Run `./scripts/test-integration.sh`
5. Submit a PR

## Acknowledgments

- [Qwen](https://github.com/QwenLM/Qwen2.5-Coder) for the excellent code model
- [candle](https://github.com/huggingface/candle) for native Rust inference
- [NixOS](https://nixos.org) for reproducible, atomic system management
- [Firecracker](https://firecracker-microvm.github.io) for fast, secure microVMs
- [Tailscale](https://tailscale.com) for simple mesh networking
- [Automerge](https://automerge.org) for conflict-free replicated data
- [OPA](https://www.openpolicyagent.org) for policy-as-code