#![allow(dead_code)]

use crate::config::CiConfig;
use crate::reporter::Reporter;
use crate::types::{format_duration, BuildSummary};
use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct DashboardState {
    pub reporter: Arc<Reporter>,
    pub config: CiConfig,
}

pub async fn start_dashboard(config: CiConfig, reporter: Reporter) {
    let state = DashboardState {
        reporter: Arc::new(reporter),
        config: config.clone(),
    };

    let app = Router::new()
        .route("/", get(handle_index))
        .route("/api/summary", get(handle_summary))
        .route("/api/builds", get(handle_list_builds))
        .route("/api/builds/{id}", get(handle_get_build))
        .route("/status.css", get(handle_css))
        .with_state(state);

    let addr = format!("{}:{}", config.dashboard.listen_addr, config.dashboard.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();

    tracing::info!(addr = %addr, "CI dashboard started");
    axum::serve(listener, app).await.unwrap();
}

async fn handle_index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn handle_css() -> Response {
    ([(axum::http::header::CONTENT_TYPE, "text/css")], STYLE_CSS).into_response()
}

async fn handle_summary(State(state): State<DashboardState>) -> Json<BuildSummary> {
    match state.reporter.summary() {
        Ok(s) => Json(s),
        Err(_) => Json(BuildSummary {
            total: 0,
            success: 0,
            failure: 0,
            running: 0,
            pending: 0,
            avg_duration_ms: 0.0,
        }),
    }
}

async fn handle_list_builds(State(state): State<DashboardState>) -> Json<Vec<serde_json::Value>> {
    match state.reporter.list_builds(50) {
        Ok(builds) => {
            let builds_json: Vec<serde_json::Value> = builds
                .into_iter()
                .map(|b| {
                    serde_json::json!({
                        "id": b.id,
                        "pipeline_name": b.pipeline_name,
                        "status": b.status.label(),
                        "status_color": b.status.color_hex(),
                        "duration_ms": b.duration_ms,
                        "duration_formatted": format_duration(b.duration_ms),
                        "branch": b.branch,
                        "commit": b.commit,
                        "started_at": b.started_at.to_rfc3339(),
                        "finished_at": b.finished_at.map(|t| t.to_rfc3339()),
                    })
                })
                .collect();
            Json(builds_json)
        }
        Err(_) => Json(Vec::new()),
    }
}

async fn handle_get_build(
    State(state): State<DashboardState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    match state.reporter.get_build(&id) {
        Ok(Some(build)) => {
            let stages_json: Vec<serde_json::Value> = build
                .stages
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "name": s.name,
                        "status": s.status.label(),
                        "status_color": s.status.color_hex(),
                        "duration_ms": s.duration_ms,
                        "duration_formatted": format_duration(s.duration_ms),
                        "exit_code": s.exit_code,
                        "allow_failure": s.allow_failure,
                    })
                })
                .collect();

            Json(serde_json::json!({
                "id": build.id,
                "pipeline_name": build.pipeline_name,
                "status": build.status.label(),
                "status_color": build.status.color_hex(),
                "duration_ms": build.duration_ms,
                "duration_formatted": format_duration(build.duration_ms),
                "branch": build.branch,
                "commit": build.commit,
                "started_at": build.started_at.to_rfc3339(),
                "finished_at": build.finished_at.map(|t| t.to_rfc3339()),
                "stages": stages_json,
            }))
        }
        Ok(None) => Json(serde_json::json!({"error": "not found"})),
        Err(_) => Json(serde_json::json!({"error": "server error"})),
    }
}

const STYLE_CSS: &str = r#"
:root { --bg: #0f0f14; --surface: #1a1a24; --surface2: #252533; --text: #d0d0dc; --text2: #8888a0; --accent: #3b82f6; }
* { margin: 0; padding: 0; box-sizing: border-box; }
body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; background: var(--bg); color: var(--text); min-height: 100vh; }
.header { background: var(--surface); border-bottom: 1px solid var(--surface2); padding: 16px 24px; display: flex; align-items: center; justify-content: space-between; }
.header h1 { font-size: 20px; font-weight: 600; }
.header .subtitle { color: var(--text2); font-size: 13px; }
.stats { display: grid; grid-template-columns: repeat(auto-fit, minmax(140px, 1fr)); gap: 12px; padding: 20px 24px; }
.stat-card { background: var(--surface); border-radius: 8px; padding: 16px; border: 1px solid var(--surface2); }
.stat-card .value { font-size: 28px; font-weight: 700; margin-bottom: 4px; }
.stat-card .label { font-size: 12px; color: var(--text2); text-transform: uppercase; letter-spacing: 0.5px; }
.builds { padding: 0 24px 24px; }
.builds h2 { font-size: 16px; font-weight: 600; margin-bottom: 12px; }
.build-row { background: var(--surface); border-radius: 8px; padding: 14px 16px; margin-bottom: 8px; border: 1px solid var(--surface2); display: flex; align-items: center; gap: 16px; cursor: pointer; transition: background .15s; }
.build-row:hover { background: var(--surface2); }
.status-dot { width: 10px; height: 10px; border-radius: 50%; flex-shrink: 0; }
.build-info { flex: 1; }
.build-id { font-weight: 600; font-size: 14px; font-family: monospace; }
.build-meta { font-size: 12px; color: var(--text2); margin-top: 2px; }
.build-duration { font-size: 13px; font-family: monospace; color: var(--text2); }
.modal-backdrop { display: none; position: fixed; inset: 0; background: rgba(0,0,0,0.6); z-index: 100; }
.modal-backdrop.active { display: block; }
.modal { display: none; position: fixed; top: 50%; left: 50%; transform: translate(-50%, -50%); background: var(--surface); border-radius: 12px; border: 1px solid var(--surface2); padding: 24px; z-index: 101; width: 90%; max-width: 720px; max-height: 80vh; overflow-y: auto; }
.modal.active { display: block; }
.modal h2 { font-size: 18px; margin-bottom: 16px; }
.modal .close { float: right; background: none; border: none; color: var(--text2); font-size: 24px; cursor: pointer; }
.modal .close:hover { color: var(--text); }
.stage { margin-bottom: 16px; border: 1px solid var(--surface2); border-radius: 8px; overflow: hidden; }
.stage-header { padding: 10px 14px; font-weight: 600; font-size: 14px; display: flex; align-items: center; gap: 10px; }
.badge { display: inline-block; padding: 2px 8px; border-radius: 4px; font-size: 11px; font-weight: 600; }
.loader { text-align: center; padding: 40px; color: var(--text2); }
"#;

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Continuum CI</title>
<link rel="stylesheet" href="/status.css">
</head>
<body>
<div class="header">
  <div><h1>Continuum CI</h1><div class="subtitle">Local Continuous Integration</div></div>
  <div id="pipeline-name" class="subtitle"></div>
</div>
<div class="stats" id="stats"></div>
<div class="builds">
  <h2>Recent Builds</h2>
  <div id="builds-list"></div>
</div>
<div class="modal-backdrop" id="backdrop" onclick="closeModal()"></div>
<div class="modal" id="modal">
  <button class="close" onclick="closeModal()">&times;</button>
  <div id="modal-content"></div>
</div>
<script>
const API = '';
async function f(u) { const r = await fetch(u); return r.json(); }
function badge(s) {
  const c = {'Success':'#22c55e','Failure':'#ef4444','Running':'#3b82f6','Pending':'#f59e0b','Cancelled':'#6b7280','Timeout':'#f97316'};
  return '<span class="badge" style="background:'+(c[s]||'#888')+'22;color:'+(c[s]||'#888')+'">'+s+'</span>';
}
function dur(ms) {
  if (ms<1000) return ms+'ms';
  if (ms<60000) return (ms/1000).toFixed(1)+'s';
  const m=Math.floor(ms/60000),s=Math.floor((ms%60000)/1000);
  return m+'m '+s+'s';
}
function tim(t) { return t ? new Date(t).toLocaleString() : '—'; }
async function loadStats() {
  const s = await f(API+'/api/summary');
  document.getElementById('stats').innerHTML = `
    <div class="stat-card"><div class="value" style="color:#22c55e">${s.success||0}</div><div class="label">Success</div></div>
    <div class="stat-card"><div class="value" style="color:#ef4444">${s.failure||0}</div><div class="label">Failure</div></div>
    <div class="stat-card"><div class="value" style="color:#3b82f6">${s.running||0}</div><div class="label">Running</div></div>
    <div class="stat-card"><div class="value" style="color:#f59e0b">${s.pending||0}</div><div class="label">Pending</div></div>
    <div class="stat-card"><div class="value">${s.total||0}</div><div class="label">Total Builds</div></div>`;
}
async function loadBuilds() {
  const b = await f(API+'/api/builds');
  document.getElementById('builds-list').innerHTML = b.map(x => `
    <div class="build-row" onclick="showBuild('${x.id}')">
      <div class="status-dot" style="background:${x.status_color}"></div>
      <div class="build-info"><div class="build-id">${x.id}</div><div class="build-meta">${x.pipeline_name} · ${x.branch||'?'} · ${tim(x.started_at)}</div></div>
      <div>${badge(x.status)}</div>
      <div class="build-duration">${x.duration_formatted}</div>
    </div>`).join('');
}
async function showBuild(id) {
  const b = await f(API+'/api/builds/'+id);
  if (!b.id) return;
  document.getElementById('backdrop').classList.add('active');
  document.getElementById('modal').classList.add('active');
  document.getElementById('modal-content').innerHTML = `
    <div style="margin-bottom:16px">
      <div style="font-size:24px;font-weight:700;margin-bottom:4px">${b.id} ${badge(b.status)}</div>
      <div style="color:var(--text2);font-size:13px">${b.pipeline_name} · ${b.branch||'?'} · ${b.commit||''} · ${tim(b.started_at)} · ${b.duration_formatted}</div>
    </div>
    ${(b.stages||[]).map(s => `
      <div class="stage">
        <div class="stage-header">
          <span class="status-dot" style="background:${s.status_color}"></span>
          ${s.name}
          <span style="margin-left:auto;font-size:12px;color:var(--text2)">${badge(s.status)} ${s.duration_formatted}</span>
        </div>
      </div>`).join('')}`;
}
function closeModal() {
  document.getElementById('backdrop').classList.remove('active');
  document.getElementById('modal').classList.remove('active');
}
setInterval(()=>{loadStats();loadBuilds();},3000);
loadStats();loadBuilds();
</script>
</body>
</html>"#;
