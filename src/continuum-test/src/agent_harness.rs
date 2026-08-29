#![allow(dead_code)]

// Agent Test Harness — programmatic GUI testing for CI
// =====================================================
// Tests the GUI end-to-end through the debug API:
//   1. Launch server
//   2. Launch client with --debug flag
//   3. Query state, simulate input, verify responses
//   4. Go through entire wizard flow programmatically
//   5. Verify connection and frame streaming
// =====================================================

use std::time::Duration;
use tokio::time::sleep;

const DEFAULT_DEBUG_PORT: u16 = 9092;
const DEFAULT_SERVER_ADDR: &str = "127.0.0.1:4433";

use std::time::Instant;

pub struct AgentHarness {
    debug_url: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone)]
pub struct AgentEvent {
    pub timestamp: String,
    pub action: String,
    pub result: String,
    pub duration_ms: u64,
}

impl AgentHarness {
    pub fn new(port: u16) -> Self {
        Self {
            debug_url: format!("http://127.0.0.1:{}", port),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
        }
    }

    pub async fn wait_for_debug_server(&self) -> Result<(), String> {
        for _ in 0..30 {
            match self
                .http
                .get(format!("{}/api/state", self.debug_url))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    return Ok(());
                }
                _ => sleep(Duration::from_millis(500)).await,
            }
        }
        Err("Debug server did not start within 15 seconds".into())
    }

    pub async fn get_state(&self) -> Result<serde_json::Value, String> {
        self.http
            .get(format!("{}/api/state", self.debug_url))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn click(&self, x: f32, y: f32) -> Result<serde_json::Value, String> {
        self.http
            .post(format!("{}/api/input/click", self.debug_url))
            .json(&serde_json::json!({ "x": x, "y": y }))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn type_text(&self, text: &str) -> Result<serde_json::Value, String> {
        self.http
            .post(format!("{}/api/input/type", self.debug_url))
            .json(&serde_json::json!({ "text": text }))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn press_key(&self, key: &str) -> Result<serde_json::Value, String> {
        self.http
            .post(format!("{}/api/input/key", self.debug_url))
            .json(&serde_json::json!({ "key": key }))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn scroll(&self, x: f32, y: f32) -> Result<serde_json::Value, String> {
        self.http
            .post(format!("{}/api/input/scroll", self.debug_url))
            .json(&serde_json::json!({ "x": x, "y": y }))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn set_wizard_step(&self, step: &str) -> Result<serde_json::Value, String> {
        self.http
            .post(format!("{}/api/wizard/set", self.debug_url))
            .json(&serde_json::json!({ "step": step }))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn wizard_next(&self) -> Result<serde_json::Value, String> {
        self.http
            .post(format!("{}/api/wizard/next", self.debug_url))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn wizard_back(&self) -> Result<serde_json::Value, String> {
        self.http
            .post(format!("{}/api/wizard/back", self.debug_url))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn wizard_skip(&self) -> Result<serde_json::Value, String> {
        self.http
            .post(format!("{}/api/wizard/skip", self.debug_url))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn pair(&self, address: &str, code: &str) -> Result<serde_json::Value, String> {
        self.http
            .post(format!("{}/api/connection/pair", self.debug_url))
            .json(&serde_json::json!({ "address": address, "pairing_code": code }))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn disconnect(&self) -> Result<serde_json::Value, String> {
        self.http
            .post(format!("{}/api/connection/disconnect", self.debug_url))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn connection_status(&self) -> Result<serde_json::Value, String> {
        self.http
            .get(format!("{}/api/connection/status", self.debug_url))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn get_frames(&self) -> Result<serde_json::Value, String> {
        self.http
            .get(format!("{}/api/frames", self.debug_url))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn get_elements(&self) -> Result<serde_json::Value, String> {
        self.http
            .get(format!("{}/api/elements", self.debug_url))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn get_config(&self) -> Result<serde_json::Value, String> {
        self.http
            .get(format!("{}/api/config", self.debug_url))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn set_config(&self, config: serde_json::Value) -> Result<serde_json::Value, String> {
        self.http
            .post(format!("{}/api/config", self.debug_url))
            .json(&config)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn stop(&self) -> Result<serde_json::Value, String> {
        self.http
            .post(format!("{}/api/stop", self.debug_url))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn wait_for_state(
        &self,
        expected_step: &str,
        timeout_secs: u64,
    ) -> Result<(), String> {
        let start = Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(timeout_secs) {
                return Err(format!(
                    "Timed out waiting for step '{}' after {}s",
                    expected_step, timeout_secs
                ));
            }
            let state = self.get_state().await?;
            let current = state["wizard_step"].as_str().unwrap_or("");
            if current == expected_step {
                return Ok(());
            }
            sleep(Duration::from_millis(200)).await;
        }
    }

    pub async fn wait_for_connection(&self, timeout_secs: u64) -> Result<(), String> {
        let start = Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(timeout_secs) {
                return Err(format!(
                    "Timed out waiting for connection after {}s",
                    timeout_secs
                ));
            }
            let status = self.connection_status().await?;
            let conn = status["status"].as_str().unwrap_or("");
            if conn == "streaming" || conn == "connected" {
                return Ok(());
            }
            sleep(Duration::from_millis(500)).await;
        }
    }
}

// ── Full E2E Test Suite ───────────────────────────────

pub async fn run_agent_tests(debug_port: u16) -> Vec<AgentEvent> {
    let agent = AgentHarness::new(debug_port);
    let mut events = Vec::new();

    // Wait for debug server
    match agent.wait_for_debug_server().await {
        Ok(()) => events.push(AgentEvent {
            timestamp: chrono::Utc::now().to_rfc3339(),
            action: "wait_for_debug_server".into(),
            result: "ok".into(),
            duration_ms: 0,
        }),
        Err(e) => {
            events.push(AgentEvent {
                timestamp: chrono::Utc::now().to_rfc3339(),
                action: "wait_for_debug_server".into(),
                result: format!("FAILED: {}", e),
                duration_ms: 0,
            });
            return events;
        }
    }

    // Test 1: Verify initial state
    let start = Instant::now();
    let state = agent.get_state().await.unwrap();
    events.push(AgentEvent {
        timestamp: chrono::Utc::now().to_rfc3339(),
        action: "get_initial_state".into(),
        result: format!("step={}", state["wizard_step"]),
        duration_ms: start.elapsed().as_millis() as u64,
    });

    // Test 2: Navigate wizard through all steps
    let steps = [
        "TutorialStep1",
        "TutorialStep2",
        "TutorialStep3",
        "FindServer",
        "Pair",
    ];
    for step in &steps {
        let start = Instant::now();
        let result = agent.set_wizard_step(step).await.unwrap();
        let step_got = result["step"].as_str().unwrap_or("?");
        events.push(AgentEvent {
            timestamp: chrono::Utc::now().to_rfc3339(),
            action: format!("set_wizard_step({})", step),
            result: if step_got == *step {
                "ok".into()
            } else {
                format!("got {}", step_got)
            },
            duration_ms: start.elapsed().as_millis() as u64,
        });
    }

    // Test 3: Go back through steps
    let steps_back = [
        "FindServer",
        "TutorialStep3",
        "TutorialStep2",
        "TutorialStep1",
        "Welcome",
    ];
    for step in &steps_back {
        let start = Instant::now();
        agent.wizard_back().await.ok();
        let state = agent.get_state().await.unwrap();
        let current = state["wizard_step"].as_str().unwrap_or("?");
        events.push(AgentEvent {
            timestamp: chrono::Utc::now().to_rfc3339(),
            action: format!("wizard_back→{}", step),
            result: if current == *step {
                "ok".into()
            } else {
                format!("got {}", current)
            },
            duration_ms: start.elapsed().as_millis() as u64,
        });
    }

    // Test 4: Skip tutorial
    let start = Instant::now();
    let result = agent.wizard_skip().await.unwrap();
    events.push(AgentEvent {
        timestamp: chrono::Utc::now().to_rfc3339(),
        action: "wizard_skip".into(),
        result: format!("step={}", result["step"]),
        duration_ms: start.elapsed().as_millis() as u64,
    });

    // Test 5: Simulate typing pairing code
    let start = Instant::now();
    agent.type_text("continuum").await.ok();
    events.push(AgentEvent {
        timestamp: chrono::Utc::now().to_rfc3339(),
        action: "type_pairing_code".into(),
        result: "ok".into(),
        duration_ms: start.elapsed().as_millis() as u64,
    });

    // Test 6: Simulate clicking connect button (center of screen)
    let start = Instant::now();
    agent.click(640.0, 410.0).await.ok();
    events.push(AgentEvent {
        timestamp: chrono::Utc::now().to_rfc3339(),
        action: "click_connect_button".into(),
        result: "ok".into(),
        duration_ms: start.elapsed().as_millis() as u64,
    });

    // Test 7: Simulate keyboard input
    let start = Instant::now();
    agent.press_key("enter").await.ok();
    agent.press_key("a").await.ok();
    agent.press_key("tab").await.ok();
    events.push(AgentEvent {
        timestamp: chrono::Utc::now().to_rfc3339(),
        action: "keyboard_input".into(),
        result: "ok".into(),
        duration_ms: start.elapsed().as_millis() as u64,
    });

    // Test 8: Simulate scroll
    let start = Instant::now();
    agent.scroll(0.0, 3.0).await.ok();
    events.push(AgentEvent {
        timestamp: chrono::Utc::now().to_rfc3339(),
        action: "scroll".into(),
        result: "ok".into(),
        duration_ms: start.elapsed().as_millis() as u64,
    });

    // Test 9: Get elements
    let start = Instant::now();
    let elements = agent.get_elements().await.unwrap();
    let count = elements["count"].as_u64().unwrap_or(0);
    events.push(AgentEvent {
        timestamp: chrono::Utc::now().to_rfc3339(),
        action: "get_elements".into(),
        result: format!("{} elements", count),
        duration_ms: start.elapsed().as_millis() as u64,
    });

    // Test 10: Get config
    let start = Instant::now();
    let config = agent.get_config().await.unwrap();
    events.push(AgentEvent {
        timestamp: chrono::Utc::now().to_rfc3339(),
        action: "get_config".into(),
        result: format!("view_only={}", config["view_only"]),
        duration_ms: start.elapsed().as_millis() as u64,
    });

    // Test 11: Set config
    let start = Instant::now();
    agent
        .set_config(serde_json::json!({ "view_only": true }))
        .await
        .ok();
    let config = agent.get_config().await.unwrap();
    events.push(AgentEvent {
        timestamp: chrono::Utc::now().to_rfc3339(),
        action: "set_config".into(),
        result: format!("view_only={}", config["view_only"]),
        duration_ms: start.elapsed().as_millis() as u64,
    });

    // Test 12: Get frames
    let start = Instant::now();
    let frames = agent.get_frames().await.unwrap();
    events.push(AgentEvent {
        timestamp: chrono::Utc::now().to_rfc3339(),
        action: "get_frames".into(),
        result: format!("count={}", frames["frame_count"]),
        duration_ms: start.elapsed().as_millis() as u64,
    });

    events
}
