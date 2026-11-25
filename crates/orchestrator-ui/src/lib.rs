//! UI: TUI and Web interfaces

pub mod tui;
pub mod web;
pub mod approval;

pub use tui::TuiApp;
pub use web::WebServer;
pub use approval::{ApprovalRequest, ApprovalResponse, UiServer};