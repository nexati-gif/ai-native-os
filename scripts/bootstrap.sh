#!/usr/bin/env bash
# AI-Native OS Bootstrap Script
# Run this on a fresh NixOS installation to set up the orchestrator

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[INFO]${NC} $*"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $*"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $*"; }

# Check if running as root
if [[ $EUID -ne 0 ]]; then
    log_error "This script must be run as root (use sudo)"
    exit 1
fi

# Detect hostname
HOSTNAME=$(hostname)
log_info "Bootstrapping AI-Native OS for host: $HOSTNAME"

# Check if flake exists
if [[ ! -f "$REPO_ROOT/nix/flake.nix" ]]; then
    log_error "Flake not found at $REPO_ROOT/nix/flake.nix"
    exit 1
fi

# Enable flakes if not already
if ! grep -q "experimental-features.*flakes" /etc/nix/nix.conf 2>/dev/null; then
    log_info "Enabling flakes in nix.conf"
    echo "experimental-features = nix-command flakes" >> /etc/nix/nix.conf
    systemctl restart nix-daemon
fi

# Build and switch to configuration
log_info "Building NixOS configuration for $HOSTNAME..."
nixos-rebuild switch --flake "$REPO_ROOT/nix#$HOSTNAME" --show-trace

# Create orchestrator user if not exists
if ! id "orchestrator" &>/dev/null; then
    log_info "Creating orchestrator user"
    useradd -r -s /bin/bash -d /var/lib/orchestrator -G kvm,systemd-journal orchestrator
fi

# Create directories
log_info "Creating orchestrator directories"
mkdir -p /var/lib/orchestrator/{models,lora,rag,federation}
mkdir -p /etc/orchestrator/policies
chown -R orchestrator:orchestrator /var/lib/orchestrator /etc/orchestrator

# Download model if not present
MODEL_PATH="/var/lib/orchestrator/models/qwen2.5-coder-32b-4bit.gguf"
if [[ ! -f "$MODEL_PATH" ]]; then
    log_info "Downloading Qwen2.5-Coder-32B-4bit model..."
    MODEL_URL="https://huggingface.co/Qwen/Qwen2.5-Coder-32B-Instruct-GGUF/resolve/main/qwen2.5-coder-32b-instruct-q4_k_m.gguf"
    sudo -u orchestrator curl -L -o "$MODEL_PATH" "$MODEL_URL" || {
        log_warn "Model download failed, you can download manually later"
    }
fi

# Download smaller model for phone
PHONE_MODEL_PATH="/var/lib/orchestrator/models/qwen2.5-coder-7b-4bit.gguf"
if [[ ! -f "$PHONE_MODEL_PATH" ]] && [[ "$HOSTNAME" == "phone" ]]; then
    log_info "Downloading Qwen2.5-Coder-7B-4bit model for mobile..."
    PHONE_MODEL_URL="https://huggingface.co/Qwen/Qwen2.5-Coder-7B-Instruct-GGUF/resolve/main/qwen2.5-coder-7b-instruct-q4_k_m.gguf"
    sudo -u orchestrator curl -L -o "$PHONE_MODEL_PATH" "$PHONE_MODEL_URL" || {
        log_warn "Phone model download failed"
    }
fi

# Initialize federation repo
log_info "Initializing federation repository"
sudo -u orchestrator bash -c "
    cd /etc/orchestrator/federation
    git init 2>/dev/null || true
    git config user.name 'orchestrator'
    git config user.email 'orchestrator@$HOSTNAME'
    mkdir -p proposals
    echo '# AI-Native OS Federation' > README.md
    git add .
    git commit -m 'Initial commit' 2>/dev/null || true
"

# Install default policies
log_info "Installing default policies"
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

chown -R root:orchestrator /etc/orchestrator/policies
chmod 750 /etc/orchestrator/policies
chmod 640 /etc/orchestrator/policies/*

# Generate config
log_info "Generating orchestrator config"
cat > /etc/orchestrator/config.toml <<EOF
device_id = "$HOSTNAME"
model_path = "/var/lib/orchestrator/models/qwen2.5-coder-32b-4bit.gguf"
rag_path = "/var/lib/orchestrator/rag"
lora_path = "/var/lib/orchestrator/lora"
state_db_path = "/var/lib/orchestrator/state.redb"
idle_timeout_secs = 30

passive_mode = { 
  enabled = true, 
  poll_interval_ms = 100, 
  events_buffer_size = 10000, 
  persist_interval_secs = 60, 
  collectors = [
    { name = "scheduler", enabled = true, config = {} },
    { name = "network", enabled = true, config = {} },
    { name = "block", enabled = true, config = {} },
    { name = "memory", enabled = true, config = {} },
  ]
}

active_mode = { 
  max_concurrent_cycles = 1, 
  proposal_timeout_secs = 300, 
  sandbox_timeout_secs = 600, 
  validation_timeout_secs = 300, 
  auto_promote_on_success = false 
}

federation = { 
  enabled = true, 
  tailscale_auth_key = null, 
  sync_interval_secs = 300, 
  automerge_doc_path = "/var/lib/orchestrator/federation.automerge", 
  git_remote = null, 
  git_branch = "main" 
}

ui = { 
  mode = "Both", 
  tui_enabled = true, 
  web_enabled = true, 
  web_port = 8080, 
  web_bind = "127.0.0.1", 
  auth_token = null 
}

policy = { 
  engine = "OpaRego", 
  policies_dir = "/etc/orchestrator/policies", 
  default_deny = true 
}

sandbox = { 
  backend = "Firecracker", 
  firecracker = { 
    vcpus = 2, 
    memory_mib = 512, 
    kernel_path = "/run/current-system/kernel", 
    initrd_path = "/run/current-system/initrd", 
    rootfs_path = "/var/lib/orchestrator/rootfs.ext4", 
    boot_args = "console=ttyS0 reboot=k panic=1 pci=off nomodules" 
  }, 
  qemu = { 
    vcpus = 2, 
    memory_mib = 1024, 
    kernel_path = "/run/current-system/kernel", 
    initrd_path = "/run/current-system/initrd", 
    disk_path = "/var/lib/orchestrator/disk.qcow2", 
    enable_kvm = true 
  }, 
  warm_pool_size = 0 
}
EOF

chown root:orchestrator /etc/orchestrator/config.toml
chmod 640 /etc/orchestrator/config.toml

# Enable and start services
log_info "Enabling and starting services"
systemctl daemon-reload
systemctl enable orchestrator.service
systemctl start orchestrator.service

# Check status
sleep 2
if systemctl is-active --quiet orchestrator.service; then
    log_success "Orchestrator service started successfully"
else
    log_error "Orchestrator service failed to start"
    systemctl status orchestrator.service
    exit 1
fi

# Setup Tailscale if auth key provided
if [[ -n "${TAILSCALE_AUTH_KEY:-}" ]]; then
    log_info "Configuring Tailscale"
    tailscale up --auth-key "$TAILSCALE_AUTH_KEY" --accept-routes
    log_success "Tailscale configured"
else
    log_warn "TAILSCALE_AUTH_KEY not set, skipping Tailscale setup"
    log_warn "Run: sudo tailscale up --auth-key <your-key> --accept-routes"
fi

# Final message
log_success "AI-Native OS bootstrap complete for $HOSTNAME!"
echo ""
echo "Next steps:"
echo "  1. Access Web UI: http://localhost:8080"
echo "  2. Run TUI: orchestrator-tui (if installed)"
echo "  3. Trigger optimization: kill -USR1 \$(pidof orchestrator)"
echo "  4. Check logs: journalctl -u orchestrator -f"
echo "  5. Configure Tailscale for federation sync"
echo ""
echo "To rebuild after changes:"
echo "  sudo nixos-rebuild switch --flake $REPO_ROOT/nix#$HOSTNAME"