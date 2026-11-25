//! Actor: executes proposals in sandbox

pub mod actor;
pub mod nix;
pub mod sandbox;
pub mod kexec;

pub use actor::{Actor, ActorConfig, ExecutionResult, SandboxBackend};
pub use sandbox::{Sandbox, FirecrackerSandbox, QemuSandbox};
pub use nix::{NixBuilder, NixOSGeneration};
pub use kexec::{KexecManager, BootEntry};