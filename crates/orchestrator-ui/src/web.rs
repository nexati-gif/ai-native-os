//! Web UI using axum + HTMX

use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::{mpsc, RwLock, oneshot};
use axum::{
    extract::{State, WebSocketUpgrade, Query},
    http::{StatusCode, HeaderMap, HeaderValue},
    response::{Html, IntoResponse, Response, Redirect},
    routing::{get, post},
    middleware::{self, Next},
    Router,
};
use tower_http::services::ServeDir;
use tower_http::cors::{CorsLayer, Any};
use orchestrator_common::{Error, Result};
use orchestrator_common::types::{Proposal, Approval, ApprovalResponse, RiskLevel, ProposalKind, Component, ProposalId};
use orchestrator_common::EventBus;
use serde::{Deserialize, Serialize};
use tracing::{info, debug, warn};
use uuid::Uuid;

/// Web server state
#[derive(Clone)]
pub struct WebState {
    pending_proposals: Arc<RwLock<HashMap<ProposalId, PendingProposal>>>,
    approval_tx: mpsc::Sender<ApprovalResponse>,
    event_bus: EventBus,
    auth_token: Option<String>,
    metrics: Arc<RwLock<WebMetrics>>,
}

#[derive(Debug, Clone)]
struct PendingProposal {
    proposal: Proposal,
    response_tx: oneshot::Sender<ApprovalResponse>,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Default, Clone)]
struct WebMetrics {
    total_requests: u64,
    active_connections: u64,
    proposals_served: u64,
    approvals_processed: u64,
}

/// Web server
pub struct WebServer {
    state: WebState,
    port: u16,
    bind_addr: String,
}

impl WebServer {
    pub async fn new(config: &orchestrator_common::types::UiConfig, event_bus: EventBus) -> Result<Self> {
        let (approval_tx, _approval_rx) = mpsc::channel(100);
        
        let state = WebState {
            pending_proposals: Arc::new(RwLock::new(HashMap::new())),
            approval_tx,
            event_bus: event_bus.clone(),
            auth_token: config.auth_token.clone(),
            metrics: Arc::new(RwLock::new(WebMetrics::default())),
        };

        Ok(Self {
            state,
            port: config.web_port,
            bind_addr: config.web_bind.clone(),
        })
    }

    pub async fn run(&self) -> Result<()> {
        let app = self.router();
        let addr = format!("{}:{}", self.bind_addr, self.port);
        info!("Web UI listening on {}", addr);
        
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        axum::serve(listener, app).await?;
        Ok(())
    }

    fn router(&self) -> Router {
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        Router::new()
            .route("/", get(Self::index))
            .route("/proposal", get(Self::get_proposal).post(Self::submit_approval))
            .route("/ws", get(Self::websocket))
            .route("/metrics", get(Self::metrics))
            .route("/health", get(Self::health))
            .nest_service("/static", ServeDir::new("static"))
            .layer(middleware::from_fn_with_state(self.state.clone(), auth_middleware))
            .layer(cors)
            .with_state(self.state.clone())
    }

    async fn index() -> Html<&'static str> {
        Html(include_str!("../static/index.html"))
    }

    async fn get_proposal(
        State(state): State<WebState>,
    ) -> Result<Html<String>, StatusCode> {
        let mut metrics = state.metrics.write().await;
        metrics.total_requests += 1;
        metrics.proposals_served += 1;
        
        let proposals = state.pending_proposals.read().await;
        if let Some((_, pending)) = proposals.iter().next() {
            Ok(Html(render_proposal_html(&pending.proposal)))
        } else {
            Err(StatusCode::NOT_FOUND)
        }
    }

    async fn submit_approval(
        State(state): State<WebState>,
        axum::Json(payload): axum::Json<ApprovalRequest>,
    ) -> Result<impl IntoResponse, StatusCode> {
        let mut proposals = state.pending_proposals.write().await;
        
        if let Some(pending) = proposals.remove(&payload.proposal_id) {
            let response = ApprovalResponse {
                proposal_id: payload.proposal_id,
                approved: payload.approved,
                modified_proposal: payload.modified_proposal,
                reason: payload.reason,
            };
            
            let _ = pending.response_tx.send(response);
            
            let mut metrics = state.metrics.write().await;
            metrics.approvals_processed += 1;
            
            // Send event for WebSocket clients
            state.event_bus.send(orchestrator_common::Event::ApprovalReceived { 
                approval: Approval {
                    proposal_id: payload.proposal_id,
                    approved: payload.approved,
                    modified_proposal: payload.modified_proposal,
                    reason: payload.reason,
                    timestamp: chrono::Utc::now(),
                    approver: "web_user".to_string(),
                }
            }).ok();
            
            Ok(StatusCode::OK)
        } else {
            Err(StatusCode::NOT_FOUND)
        }
    }

    async fn websocket(
        ws: WebSocketUpgrade,
        State(state): State<WebState>,
    ) -> Response {
        ws.on_upgrade(|socket| Self::handle_ws(socket, state))
    }

    async fn handle_ws(socket: axum::extract::ws::WebSocket, state: WebState) {
        let mut metrics = state.metrics.write().await;
        metrics.active_connections += 1;
        drop(metrics);

        let mut rx = state.event_bus.receiver();
        let (mut ws_tx, mut ws_rx) = socket.split();
        
        // Send initial state
        let proposals = state.pending_proposals.read().await;
        for (_, pending) in proposals.iter() {
            let msg = serde_json::to_string(&orchestrator_common::Event::ApprovalRequested { 
                proposal_id: pending.proposal.id 
            }).ok();
            if let Some(msg) = msg {
                ws_tx.send(axum::extract::ws::Message::Text(msg.into())).await.ok();
            }
        }
        drop(proposals);
        
        loop {
            tokio::select! {
                event = rx.recv() => {
                    if let Ok(event) = event {
                        let msg = serde_json::to_string(&event).ok();
                        if let Some(msg) = msg {
                            if ws_tx.send(axum::extract::ws::Message::Text(msg.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                msg = ws_rx.recv() => {
                    if msg.is_none() { break; }
                }
            }
        }

        let mut metrics = state.metrics.write().await;
        metrics.active_connections = metrics.active_connections.saturating_sub(1);
    }

    async fn metrics() -> Html<String> {
        // Return Prometheus metrics format
        Html("# Metrics not implemented yet\n".to_string())
    }

    async fn health() -> &'static str {
        "OK"
    }

    /// Set a new proposal for approval and wait for response
    pub async fn request_approval(&self, proposal: Proposal) -> Result<ApprovalResponse> {
        info!("Requesting approval for proposal {}", proposal.id);
        
        let (response_tx, response_rx) = oneshot::channel();
        
        let pending = PendingProposal {
            proposal: proposal.clone(),
            response_tx,
            created_at: chrono::Utc::now(),
        };
        
        // Store proposal
        self.state.pending_proposals.write().await.insert(proposal.id, pending);
        
        // Send event for WebSocket clients
        self.state.event_bus.send(orchestrator_common::Event::ApprovalRequested { proposal_id: proposal.id })?;
        
        // Wait for approval response with timeout
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(300), // 5 min timeout
            response_rx
        ).await
        .map_err(|_| Error::Timeout("Approval request timed out".to_string()))?
        .map_err(|_| Error::InvalidState("Approval channel closed".to_string()))?;
        
        Ok(response)
    }

    pub async fn shutdown(&self) {
        // Cleanup pending proposals
        let mut proposals = self.state.pending_proposals.write().await;
        for (_, pending) in proposals.drain() {
            let _ = pending.response_tx.send(ApprovalResponse {
                proposal_id: pending.proposal.id,
                approved: false,
                modified_proposal: None,
                reason: Some("Server shutting down".to_string()),
            });
        }
    }
}

/// Authentication middleware
async fn auth_middleware(
    State(state): State<WebState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Skip auth for health and static files
    let path = request.uri().path();
    if path == "/health" || path.starts_with("/static") {
        return Ok(next.run(request).await);
    }
    
    // Check auth token if configured
    if let Some(token) = &state.auth_token {
        let headers = request.headers();
        let auth_header = headers.get("Authorization")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        
        if !auth_header.starts_with("Bearer ") || &auth_header[7..] != token {
            // Check query param for websocket
            if path == "/ws" {
                let query = request.uri().query().unwrap_or("");
                if !query.contains(&format!("token={}", token)) {
                    return Err(StatusCode::UNAUTHORIZED);
                }
            } else {
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
    }
    
    Ok(next.run(request).await)
}

/// Approval request from web UI
#[derive(Debug, Deserialize, Serialize)]
pub struct ApprovalRequest {
    pub proposal_id: ProposalId,
    pub approved: bool,
    pub modified_proposal: Option<Proposal>,
    pub reason: Option<String>,
}

fn render_proposal_html(proposal: &Proposal) -> String {
    let risk_class = match proposal.risk_level {
        RiskLevel::None | RiskLevel::Low => "risk-low",
        RiskLevel::Medium => "risk-medium",
        RiskLevel::High => "risk-high",
        RiskLevel::Critical => "risk-critical",
    };

    // Format diff with syntax highlighting
    let diff_html = format_diff_html(proposal.diff.as_deref().unwrap_or("No diff provided"));

    format!(r#"
<!DOCTYPE html>
<html>
<head>
    <title>Proposal Approval - AI-Native OS</title>
    <script src="https://unpkg.com/htmx.org@1.9.10"></script>
    <style>
        body {{ font-family: system-ui, sans-serif; max-width: 1000px; margin: 2rem auto; padding: 1rem; background: #fafafa; }}
        .card {{ background: white; border: 1px solid #e5e5e5; border-radius: 8px; padding: 1.5rem; margin-bottom: 1rem; box-shadow: 0 1px 3px rgba(0,0,0,0.05); }}
        .risk-low {{ border-left: 4px solid #22c55e; }}
        .risk-medium {{ border-left: 4px solid #eab308; }}
        .risk-high {{ border-left: 4px solid #ef4444; }}
        .risk-critical {{ border-left: 4px solid #dc2626; border-width: 4px; }}
        .metric {{ display: inline-block; margin: 0.5rem 1.5rem 0.5rem 0; }}
        .metric-label {{ color: #666; font-size: 0.875rem; }}
        .metric-value {{ font-weight: 600; font-size: 1.125rem; }}
        .positive {{ color: #22c55e; }}
        .negative {{ color: #ef4444; }}
        .neutral {{ color: #3b82f6; }}
        button {{ padding: 0.75rem 1.5rem; margin-right: 1rem; border: none; border-radius: 6px; cursor: pointer; font-size: 1rem; font-weight: 500; transition: opacity 0.2s; }}
        button:hover {{ opacity: 0.9; }}
        button:disabled {{ opacity: 0.5; cursor: not-allowed; }}
        .btn-approve {{ background: #22c55e; color: white; }}
        .btn-reject {{ background: #ef4444; color: white; }}
        .btn-modify {{ background: #3b82f6; color: white; }}
        pre {{ background: #1e1e1e; color: #d4d4d4; padding: 1rem; border-radius: 6px; overflow-x: auto; font-size: 0.875rem; line-height: 1.5; }}
        .diff-add {{ color: #4ec9b0; }}
        .diff-remove {{ color: #f44747; }}
        .diff-context {{ color: #9cdcfe; }}
        .diff-header {{ color: #dcdcaa; font-weight: bold; }}
        .section-title {{ font-size: 1.125rem; font-weight: 600; margin-bottom: 0.75rem; color: #1f1f1f; }}
        .badge {{ display: inline-block; padding: 0.25rem 0.5rem; border-radius: 4px; font-size: 0.75rem; font-weight: 600; text-transform: uppercase; }}
        .badge-risk-none {{ background: #dcfce7; color: #166534; }}
        .badge-risk-low {{ background: #dcfce7; color: #166534; }}
        .badge-risk-medium {{ background: #fef9c3; color: #854d0e; }}
        .badge-risk-high {{ background: #fee2e2; color: #991b1b; }}
        .badge-risk-critical {{ background: #fee2e2; color: #991b1b; }}
        .grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 1rem; }}
        .form-actions {{ display: flex; gap: 1rem; margin-top: 1.5rem; padding-top: 1.5rem; border-top: 1px solid #e5e5e5; }}
        .loading {{ text-align: center; padding: 2rem; color: #666; }}
    </style>
</head>
<body>
    <div class="card {risk_class}">
        <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 1rem;">
            <h1 style="margin: 0;">Proposal: {}</h1>
            <span class="badge badge-risk-{:?}">{:?}</span>
        </div>
        <p><strong>ID:</strong> <code>{}</code></p>
        <p><strong>Type:</strong> {:?}</p>
        <p><strong>Trigger:</strong> {:?}</p>
        
        <h2 class="section-title">Rationale</h2>
        <p>{}</p>
        
        <h2 class="section-title">Estimated Impact</h2>
        <div class="grid">
            <div class="metric">
                <div class="metric-label">Performance</div>
                <div class="metric-value {}">{:+.1}%</div>
            </div>
            <div class="metric">
                <div class="metric-label">CPU Usage</div>
                <div class="metric-value {}">{:+.1}%</div>
            </div>
            <div class="metric">
                <div class="metric-label">Memory</div>
                <div class="metric-value {}">{:+} MiB</div>
            </div>
            <div class="metric">
                <div class="metric-label">Latency</div>
                <div class="metric-value {}">{:+.1}%</div>
            </div>
            <div class="metric">
                <div class="metric-label">Battery</div>
                <div class="metric-value {}">{:+.1}%</div>
            </div>
            <div class="metric">
                <div class="metric-label">Stability Risk</div>
                <div class="metric-value">{:?}</div>
            </div>
            <div class="metric">
                <div class="metric-label">Security Risk</div>
                <div class="metric-value">{:?}</div>
            </div>
            <div class="metric">
                <div class="metric-label">Compat Risk</div>
                <div class="metric-value">{:?}</div>
            </div>
        </div>
        
        <h2 class="section-title">Affected Components</h2>
        <p>{}</p>
        
        <h2 class="section-title">Diff</h2>
        <pre>{}</pre>
        
        <h2 class="section-title">Test Plan</h2>
        <div class="grid">
            <div class="metric">
                <div class="metric-label">Canary Duration</div>
                <div class="metric-value">{}s</div>
            </div>
            <div class="metric">
                <div class="metric-label">Min Improvement</div>
                <div class="metric-value">{:.0}%</div>
            </div>
            <div class="metric">
                <div class="metric-label">Max Regression</div>
                <div class="metric-value">{:.0}%</div>
            </div>
            <div class="metric">
                <div class="metric-label">Significance (p)</div>
                <div class="metric-value">{:.2}</div>
            </div>
        </div>
        
        <div class="form-actions">
            <form hx-post="/proposal" hx-target="body" hx-swap="outerHTML" hx-indicator=".htmx-indicator">
                <input type="hidden" name="proposal_id" value="{}">
                <button type="submit" name="approved" value="true" class="btn-approve">✓ Approve</button>
                <button type="submit" name="approved" value="false" class="btn-reject">✗ Reject</button>
                <button type="submit" name="action" value="modify" class="btn-modify">✎ Modify</button>
            </form>
        </div>
    </div>
    
    <script>
        // Auto-refresh every 30 seconds if no proposal
        setTimeout(() => location.reload(), 30000);
    </script>
</body>
</html>
"#,
        proposal.description,
        proposal.risk_level,
        proposal.risk_level,
        proposal.id,
        proposal.kind,
        proposal.trigger,
        proposal.rationale,
        if proposal.estimated_impact.performance_delta_pct >= 0.0 { "positive" } else { "negative" },
        proposal.estimated_impact.performance_delta_pct,
        if proposal.estimated_impact.cpu_delta_pct <= 0.0 { "positive" } else { "negative" },
        proposal.estimated_impact.cpu_delta_pct,
        if proposal.estimated_impact.memory_delta_mib <= 0 { "positive" } else { "negative" },
        proposal.estimated_impact.memory_delta_mib,
        if proposal.estimated_impact.latency_delta_pct <= 0.0 { "positive" } else { "negative" },
        proposal.estimated_impact.latency_delta_pct,
        if proposal.estimated_impact.battery_delta_pct <= 0.0 { "positive" } else { "negative" },
        proposal.estimated_impact.battery_delta_pct,
        proposal.estimated_impact.stability_risk,
        proposal.estimated_impact.security_risk,
        proposal.estimated_impact.compatibility_risk,
        proposal.affected_components.iter().map(|c| format!("{:?}", c)).collect::<Vec<_>>().join(", "),
        diff_html,
        proposal.test_plan.canary_duration_secs,
        proposal.test_plan.min_improvement_pct,
        proposal.test_plan.max_regression_pct,
        proposal.test_plan.statistical_significance,
        proposal.id,
    )
}

fn format_diff_html(diff: &str) -> String {
    diff.lines()
        .map(|line| {
            let escaped = html_escape::encode_text(line);
            if line.starts_with('+') {
                format!("<span class=\"diff-add\">{}</span>", escaped)
            } else if line.starts_with('-') {
                format!("<span class=\"diff-remove\">{}</span>", escaped)
            } else if line.starts_with("@@") {
                format!("<span class=\"diff-header\">{}</span>", escaped)
            } else {
                format!("<span class=\"diff-context\">{}</span>", escaped)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}