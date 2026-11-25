//! AI-Native OS Orchestrator - Main Entry Point

use std::path::PathBuf;
use clap::Parser;
use tracing::{info, error};
use orchestrator_core::{OrchestratorLoop, SignalHandler};
use orchestrator_common::{Result, load_orchestrator_config, init_tracing};

#[derive(Parser, Debug)]
#[command(name = "orchestrator", version, about = "AI-Native OS Orchestrator")]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "/etc/orchestrator/config.toml")]
    config: PathBox,
    
    /// Run in foreground (don't daemonize)
    #[arg(short, long)]
    foreground: bool,
    
    /// Validate config and exit
    #[arg(long)]
    validate_config: bool,
    
    /// Print default config and exit
    #[arg(long)]
    print_default_config: bool,
}

#[derive(Debug, Clone)]
struct PathBox(PathBuf);

impl std::str::FromStr for PathBox {
    type Err = std::io::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(PathBuf::from(s)))
    }
}

impl std::ops::Deref for PathBox {
    type Target = PathBuf;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    
    // Initialize tracing
    init_tracing()?;
    
    info!("Starting AI-Native OS Orchestrator v{}", env!("CARGO_PKG_VERSION"));
    
    // Handle special flags
    if args.print_default_config {
        print_default_config();
        return Ok(());
    }
    
    // Load configuration
    let config = load_orchestrator_config(&args.config)?;
    
    if args.validate_config {
        info!("Configuration validated successfully");
        return Ok(());
    }
    
    // Create orchestrator loop
    let mut orchestrator = OrchestratorLoop::new(config).await?;
    
    // Install signal handlers
    let mut signals = SignalHandler::new()?;
    signals.install().await?;
    
    // Start passive observation loop
    orchestrator.start_passive_loop().await?;
    
    info!("Orchestrator started successfully. Passive observation running.");
    info!("Send SIGUSR1 to trigger optimization cycle, SIGUSR2 to dump state");
    
    // Main event loop
    loop {
        tokio::select! {
            signal = signals.wait_shutdown() => {
                match signal {
                    orchestrator_core::ShutdownSignal::SigTerm | orchestrator_core::ShutdownSignal::SigInt => {
                        info!("Shutdown signal received");
                        break;
                    }
                    orchestrator_core::ShutdownSignal::SigUsr1 => {
                        info!("SIGUSR1 received - triggering optimization cycle");
                        if let Err(e) = orchestrator.active_cycle(orchestrator_common::types::Trigger::Manual { 
                            reason: "SIGUSR1".to_string() 
                        }).await {
                            error!("Active cycle failed: {}", e);
                        }
                    }
                    orchestrator_core::ShutdownSignal::SigUsr2 => {
                        info!("SIGUSR2 received - dumping state");
                        orchestrator.persist_state().await.ok();
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl+C received");
                break;
            }
        }
    }
    
    // Graceful shutdown
    info!("Shutting down...");
    orchestrator.shutdown().await?;
    
    info!("Orchestrator stopped");
    Ok(())
}

fn print_default_config() {
    println!(include_str!("../../config.example.toml"));
}