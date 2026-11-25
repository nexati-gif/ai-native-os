//! Signal handling for graceful shutdown

use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;
use tracing::{info, warn};
use crate::{Error, Result};

/// Signal handler for graceful shutdown
pub struct SignalHandler {
    shutdown_tx: mpsc::Sender<ShutdownSignal>,
    shutdown_rx: mpsc::Receiver<ShutdownSignal>,
}

#[derive(Debug, Clone, Copy)]
pub enum ShutdownSignal {
    SigTerm,
    SigInt,
    SigUsr1,  // Custom: trigger active cycle
    SigUsr2,  // Custom: dump state
}

impl SignalHandler {
    pub fn new() -> Result<Self> {
        let (tx, rx) = mpsc::channel(16);
        Ok(Self { shutdown_tx: tx, shutdown_rx: rx })
    }

    /// Install signal handlers
    pub async fn install(&self) -> Result<()> {
        let tx = self.shutdown_tx.clone();
        
        // SIGTERM
        let mut sigterm = signal(SignalKind::terminate())?;
        let tx_term = tx.clone();
        tokio::spawn(async move {
            sigterm.recv().await;
            info!("Received SIGTERM");
            tx_term.send(ShutdownSignal::SigTerm).await.ok();
        });

        // SIGINT (Ctrl+C)
        let mut sigint = signal(SignalKind::interrupt())?;
        let tx_int = tx.clone();
        tokio::spawn(async move {
            sigint.recv().await;
            info!("Received SIGINT");
            tx_int.send(ShutdownSignal::SigInt).await.ok();
        });

        // SIGUSR1 - trigger optimization cycle
        let mut sigusr1 = signal(SignalKind::user_defined1())?;
        let tx_usr1 = tx.clone();
        tokio::spawn(async move {
            sigusr1.recv().await;
            info!("Received SIGUSR1 - triggering optimization cycle");
            tx_usr1.send(ShutdownSignal::SigUsr1).await.ok();
        });

        // SIGUSR2 - dump state
        let mut sigusr2 = signal(SignalKind::user_defined2())?;
        let tx_usr2 = tx.clone();
        tokio::spawn(async move {
            sigusr2.recv().await;
            info!("Received SIGUSR2 - dumping state");
            tx_usr2.send(ShutdownSignal::SigUsr2).await.ok();
        });

        Ok(())
    }

    /// Wait for shutdown signal
    pub async fn wait_shutdown(&mut self) -> ShutdownSignal {
        self.shutdown_rx.recv().await.unwrap_or(ShutdownSignal::SigTerm)
    }

    /// Try to receive a signal without blocking
    pub fn try_recv(&mut self) -> Option<ShutdownSignal> {
        self.shutdown_rx.try_recv().ok()
    }

    /// Get sender for manual signal injection (testing)
    pub fn sender(&self) -> mpsc::Sender<ShutdownSignal> {
        self.shutdown_tx.clone()
    }
}

impl Default for SignalHandler {
    fn default() -> Self {
        Self::new().expect("Failed to create signal handler")
    }
}