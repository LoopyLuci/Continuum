#[cfg(test)]
mod tests {
    use super::super::cdp::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn make_test_state() -> DebugState {
        DebugState {
            state: Arc::new(parking_lot::RwLock::new(AppStateSnapshot {
                app_mode: "connected".into(),
                wizard_step: "Done".into(),
                connection_status: "streaming".into(),
                server_address: Some("127.0.0.1:4433".into()),
                machine_id: "abc123".into(),
                pairing_code: "test12".into(),
                fps: 30.0,
                frame_count: 42,
                bytes_received: 1024,
                uptime_secs: 60,
                elements: vec![UiElement {
                    node_id: "1".into(),
                    tag: "BUTTON".into(),
                    bounds: ElementBounds {
                        x: 10.0,
                        y: 20.0,
                        width: 100.0,
                        height: 30.0,
                    },
                    visible: true,
                    focused: false,
                    text: Some("Connect".into()),
                    children: vec![],
                }],
                errors: vec![],
            })),
            event_tx: tokio::sync::broadcast::channel(256).0,
        }
    }

    #[tokio::test]
    async fn test_json_endpoint_returns_list() {
        let state = make_test_state();
        let app = axum::Router::new()
            .route("/json", axum::routing::get(handle_json_list))
            .with_state(state);

        let req = Request::builder().uri("/json").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.is_array());
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "continuum");
        assert_eq!(arr[0]["type"], "page");
    }

    #[tokio::test]
    async fn test_version_endpoint() {
        let state = make_test_state();
        let app = axum::Router::new()
            .route("/json/version", axum::routing::get(handle_version))
            .with_state(state);

        let req = Request::builder()
            .uri("/json/version")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["Browser"].as_str().unwrap().contains("Continuum"));
        assert_eq!(json["Protocol-Version"], "1.0");
    }

    #[tokio::test]
    async fn test_protocol_endpoint() {
        let state = make_test_state();
        let app = axum::Router::new()
            .route("/json/protocol", axum::routing::get(handle_protocol))
            .with_state(state);

        let req = Request::builder()
            .uri("/json/protocol")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let domains = json["domains"].as_array().unwrap();
        assert!(domains.iter().any(|d| d["domain"] == "Page"));
        assert!(domains.iter().any(|d| d["domain"] == "Runtime"));
        assert!(domains.iter().any(|d| d["domain"] == "DOM"));
        assert!(domains.iter().any(|d| d["domain"] == "Input"));
    }

    #[tokio::test]
    async fn test_index_endpoint() {
        let state = make_test_state();
        let app = axum::Router::new()
            .route("/", axum::routing::get(handle_index))
            .with_state(state);

        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["description"]
            .as_str()
            .unwrap()
            .contains("Continuum"));
    }

    #[tokio::test]
    async fn test_cdp_command_page_enable() {
        let state = make_test_state();
        let cmd = CdpCommand {
            id: 1,
            method: "Page.enable".into(),
            params: serde_json::json!({}),
        };
        let resp = process_cdp_command(&state, &cmd).await;
        assert_eq!(resp.id, 1);
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn test_cdp_command_dom_get_document() {
        let state = make_test_state();
        let cmd = CdpCommand {
            id: 2,
            method: "DOM.getDocument".into(),
            params: serde_json::json!({}),
        };
        let resp = process_cdp_command(&state, &cmd).await;
        assert_eq!(resp.id, 2);
        assert!(resp.error.is_none());
        let root = &resp.result["root"];
        assert_eq!(root["nodeName"], "CONTINUUM");
        let children = root["children"].as_array().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0]["nodeName"], "BUTTON");
    }

    #[tokio::test]
    async fn test_cdp_command_dom_get_box_model() {
        let state = make_test_state();
        let cmd = CdpCommand {
            id: 3,
            method: "DOM.getBoxModel".into(),
            params: serde_json::json!({ "nodeId": 1 }),
        };
        let resp = process_cdp_command(&state, &cmd).await;
        assert_eq!(resp.id, 3);
        assert!(resp.error.is_none());
        let model = &resp.result["model"];
        assert!(model["width"].as_f64().unwrap() > 0.0);
    }

    #[tokio::test]
    async fn test_cdp_command_dom_get_box_model_missing_node() {
        let state = make_test_state();
        let cmd = CdpCommand {
            id: 4,
            method: "DOM.getBoxModel".into(),
            params: serde_json::json!({ "nodeId": 999 }),
        };
        let resp = process_cdp_command(&state, &cmd).await;
        assert_eq!(resp.id, 4);
        assert!(resp.error.is_none());
        assert!(resp.result["error"].as_str().is_some());
    }

    #[tokio::test]
    async fn test_cdp_command_runtime_evaluate() {
        let state = make_test_state();
        let cmd = CdpCommand {
            id: 5,
            method: "Runtime.evaluate".into(),
            params: serde_json::json!({ "expression": "1+1" }),
        };
        let resp = process_cdp_command(&state, &cmd).await;
        assert_eq!(resp.id, 5);
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn test_cdp_command_input_mouse() {
        let state = make_test_state();
        let cmd = CdpCommand {
            id: 6,
            method: "Input.dispatchMouseEvent".into(),
            params: serde_json::json!({ "type": "mousePressed", "x": 100, "y": 200 }),
        };
        let resp = process_cdp_command(&state, &cmd).await;
        assert_eq!(resp.id, 6);
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn test_cdp_command_input_key() {
        let state = make_test_state();
        let cmd = CdpCommand {
            id: 7,
            method: "Input.dispatchKeyEvent".into(),
            params: serde_json::json!({ "type": "keyDown", "key": "Enter" }),
        };
        let resp = process_cdp_command(&state, &cmd).await;
        assert_eq!(resp.id, 7);
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn test_cdp_command_unknown_method() {
        let state = make_test_state();
        let cmd = CdpCommand {
            id: 8,
            method: "Network.enable".into(),
            params: serde_json::json!({}),
        };
        let resp = process_cdp_command(&state, &cmd).await;
        assert_eq!(resp.id, 8);
        assert!(resp.error.is_none());
        assert!(resp.result["error"].as_str().unwrap().contains("Network.enable"));
    }

    #[tokio::test]
    async fn test_cdp_command_capture_screenshot() {
        let state = make_test_state();
        let cmd = CdpCommand {
            id: 9,
            method: "Page.captureScreenshot".into(),
            params: serde_json::json!({}),
        };
        let resp = process_cdp_command(&state, &cmd).await;
        assert_eq!(resp.id, 9);
        assert!(resp.error.is_none());
        assert!(resp.result["data"].as_str().unwrap().contains("42"));
    }

    #[tokio::test]
    async fn test_cdp_state_snapshot_reflects_changes() {
        let state = make_test_state();

        {
            let mut s = state.state.write();
            s.frame_count = 100;
            s.connection_status = "disconnected".into();
        }

        let cmd = CdpCommand {
            id: 10,
            method: "Page.captureScreenshot".into(),
            params: serde_json::json!({}),
        };
        let resp = process_cdp_command(&state, &cmd).await;
        assert_eq!(resp.id, 10);
        assert!(resp.error.is_none());
        assert!(resp.result["data"].as_str().unwrap().contains("100"));
    }

    #[tokio::test]
    async fn test_cdp_command_runtime_get_properties() {
        let state = make_test_state();
        let cmd = CdpCommand {
            id: 11,
            method: "Runtime.getProperties".into(),
            params: serde_json::json!({ "objectId": "obj1" }),
        };
        let resp = process_cdp_command(&state, &cmd).await;
        assert_eq!(resp.id, 11);
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn test_cdp_command_dom_enable() {
        let state = make_test_state();
        let cmd = CdpCommand {
            id: 12,
            method: "DOM.enable".into(),
            params: serde_json::json!({}),
        };
        let resp = process_cdp_command(&state, &cmd).await;
        assert_eq!(resp.id, 12);
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn test_cdp_command_runtime_enable() {
        let state = make_test_state();
        let cmd = CdpCommand {
            id: 13,
            method: "Runtime.enable".into(),
            params: serde_json::json!({}),
        };
        let resp = process_cdp_command(&state, &cmd).await;
        assert_eq!(resp.id, 13);
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn test_dom_get_document_empty_elements() {
        let state = DebugState {
            state: Arc::new(parking_lot::RwLock::new(AppStateSnapshot {
                elements: vec![],
                ..Default::default()
            })),
            event_tx: tokio::sync::broadcast::channel(256).0,
        };
        let cmd = CdpCommand {
            id: 14,
            method: "DOM.getDocument".into(),
            params: serde_json::json!({}),
        };
        let resp = process_cdp_command(&state, &cmd).await;
        assert_eq!(resp.id, 14);
        assert!(resp.error.is_none());
        let children = resp.result["root"]["children"].as_array().unwrap();
        assert_eq!(children.len(), 0);
    }
}
