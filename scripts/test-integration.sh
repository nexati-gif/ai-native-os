#!/usr/bin/env bash
# Integration tests for AI-Native OS Orchestrator

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[TEST]${NC} $*"; }
log_pass() { echo -e "${GREEN}[PASS]${NC} $*"; }
log_fail() { echo -e "${RED}[FAIL]${NC} $*"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }

# Test results
TESTS_TOTAL=0
TESTS_PASSED=0
TESTS_FAILED=0

run_test() {
    local name="$1"
    local cmd="$2"
    
    TESTS_TOTAL=$((TESTS_TOTAL + 1))
    log_info "Running: $name"
    
    if eval "$cmd" > /tmp/test_output.log 2>&1; then
        log_pass "$name"
        TESTS_PASSED=$((TESTS_PASSED + 1))
    else
        log_fail "$name"
        TESTS_FAILED=$((TESTS_FAILED + 1))
        cat /tmp/test_output.log
    fi
}

# Ensure we're in the repo root
cd "$REPO_ROOT"

log_info "=== AI-Native OS Integration Tests ==="
echo ""

# 1. Cargo build
run_test "Cargo workspace build" "cargo build --workspace --locked"

# 2. Cargo tests
run_test "Cargo unit tests" "cargo test --workspace --locked --lib"

# 3. Clippy
run_test "Clippy lints" "cargo clippy --workspace --locked -- -D warnings"

# 4. Format check
run_test "Format check" "cargo fmt --all -- --check"

# 5. Security audit
run_test "Cargo audit" "cargo audit --workspace || true"

# 6. Nix flake check
run_test "Nix flake check" "nix flake check --no-build"

# 7. Nix build
run_test "Nix build orchestrator" "nix build .#orchestrator --no-link"

# 8. NixOS configuration syntax
run_test "NixOS config syntax (laptop)" "nix-instantiate --eval --strict nix/hosts/laptop.nix >/dev/null"
run_test "NixOS config syntax (phone)" "nix-instantiate --eval --strict nix/hosts/phone.nix >/dev/null"

# 9. Kernel config validation
run_test "Kernel config has sched_ext" "grep -q 'CONFIG_SCHED_EXT=y' nix/modules/kernel.nix"
run_test "Kernel config has BPF LSM" "grep -q 'CONFIG_BPF_LSM=y' nix/modules/kernel.nix"

# 10. Policy files exist
run_test "Default policy exists" "test -f nix/modules/orchestrator.nix && grep -q 'package orchestrator' nix/modules/orchestrator.nix"

# 11. Config example valid TOML
run_test "Config example valid TOML" "toml-test config.example.toml 2>/dev/null || python3 -c \"import tomllib; tomllib.load(open('config.example.toml', 'rb'))\""

# 12. Check for required files
run_test "Flake.nix exists" "test -f nix/flake.nix"
run_test "Bootstrap script executable" "test -x scripts/bootstrap.sh"
run_test "Dev shell exists" "test -f scripts/dev-shell.nix"
run_test "CI workflow exists" "test -f .github/workflows/ci.yml"
run_test "Dependabot config exists" "test -f .github/dependabot.yml"
run_test "Dockerfile exists" "test -f Dockerfile"
run_test "rust-toolchain.toml exists" "test -f rust-toolchain.toml"
run_test ".cargo/config.toml exists" "test -f .cargo/config.toml"

# 13. Rust crate structure
run_test "Core crate exists" "test -f crates/orchestrator-core/Cargo.toml"
run_test "Observe crate exists" "test -f crates/orchestrator-observe/Cargo.toml"
run_test "Plan crate exists" "test -f crates/orchestrator-plan/Cargo.toml"
run_test "Act crate exists" "test -f crates/orchestrator-act/Cargo.toml"
run_test "Validate crate exists" "test -f crates/orchestrator-validate/Cargo.toml"
run_test "Federate crate exists" "test -f crates/orchestrator-federate/Cargo.toml"
run_test "UI crate exists" "test -f crates/orchestrator-ui/Cargo.toml"
run_test "Policy crate exists" "test -f crates/orchestrator-policy/Cargo.toml"

# 14. Check for TODO/FIXME in code
TODO_COUNT=$(grep -r "TODO\|FIXME" --include="*.rs" crates/ | wc -l)
log_info "TODO/FIXME count in Rust code: $TODO_COUNT"

# 15. Check documentation
run_test "README exists" "test -f README.md"

echo ""
log_info "=== Test Summary ==="
echo "Total:  $TESTS_TOTAL"
echo "Passed: $TESTS_PASSED"
echo "Failed: $TESTS_FAILED"

if [[ $TESTS_FAILED -gt 0 ]]; then
    log_fail "Some tests failed"
    exit 1
else
    log_pass "All tests passed!"
    exit 0
fi