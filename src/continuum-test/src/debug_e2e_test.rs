#[cfg(test)]
mod tests {
    use continuum_transport::debug_tunnel::CdpTunnelMessage;
    use tokio::sync::mpsc;

    async fn start_mock_cdp(port: u16) {
        let app = axum::Router::new()
            .route(
                "/json/version",
                axum::routing::get(|| async {
                    axum::Json(serde_json::json!({
                        "Browser": "WebView2/1.0.0",
                        "Protocol-Version": "1.0",
                        "V8-Version": "12.0",
                        "User-Agent": "Continuum-Test/1.0",
                    }))
                }),
            )
            .route(
                "/json",
                axum::routing::get(move || async move {
                    axum::Json(serde_json::json!([{
                        "id": "test-page",
                        "type": "page",
                        "url": "https://example.com",
                        "title": "Test Page",
                        "webSocketDebuggerUrl": format!("ws://127.0.0.1:{}/devtools/page/test", port),
                    }]))
                }),
            );

        let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        axum::serve(listener, app).await.unwrap();
    }

    #[tokio::test]
    async fn test_mock_cdp_endpoint() {
        let port = 19222;
        tokio::spawn(start_mock_cdp(port));

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let url = format!("http://127.0.0.1:{}/json/version", port);
        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status(), 200);

        let json: serde_json::Value = resp.json().await.unwrap();
        assert!(json["Browser"].as_str().unwrap().contains("WebView2"));
    }

    #[tokio::test]
    async fn test_cdp_tunnel_roundtrip() {
        let (tx, mut rx) = mpsc::channel(256);

        let request = CdpTunnelMessage {
            id: 1,
            method: "Page.navigate".into(),
            params: serde_json::json!({ "url": "https://example.com" }),
            is_request: true,
        };

        tx.send(request.clone()).await.unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received.id, 1);
        assert_eq!(received.method, "Page.navigate");
        assert!(received.is_request);
    }

    #[tokio::test]
    async fn test_cdp_tunnel_bidirectional() {
        let (to_server_tx, mut to_server_rx) = mpsc::channel(256);
        let (from_server_tx, mut from_server_rx) = mpsc::channel(256);

        let cmd = CdpTunnelMessage {
            id: 42,
            method: "Runtime.evaluate".into(),
            params: serde_json::json!({ "expression": "document.title" }),
            is_request: true,
        };
        to_server_tx.send(cmd).await.unwrap();

        let received = to_server_rx.recv().await.unwrap();
        let response = CdpTunnelMessage {
            id: received.id,
            method: received.method.clone(),
            params: serde_json::json!({ "result": { "type": "string", "value": "Test Page" } }),
            is_request: false,
        };
        from_server_tx.send(response).await.unwrap();

        let resp = from_server_rx.recv().await.unwrap();
        assert_eq!(resp.id, 42);
        assert!(!resp.is_request);
        assert_eq!(resp.params["result"]["value"], "Test Page");
    }

    #[tokio::test]
    async fn test_cdp_various_commands_roundtrip() {
        let commands = vec![
            ("Page.enable", serde_json::json!({})),
            (
                "Page.captureScreenshot",
                serde_json::json!({ "format": "png" }),
            ),
            (
                "Runtime.evaluate",
                serde_json::json!({ "expression": "1+1" }),
            ),
            ("DOM.getDocument", serde_json::json!({ "depth": 3 })),
            ("DOM.getBoxModel", serde_json::json!({ "nodeId": 1 })),
            (
                "Input.dispatchMouseEvent",
                serde_json::json!({ "type": "mousePressed", "x": 100, "y": 200 }),
            ),
            (
                "Input.dispatchKeyEvent",
                serde_json::json!({ "type": "keyDown", "key": "Enter" }),
            ),
            ("Network.enable", serde_json::json!({})),
            ("Debugger.enable", serde_json::json!({})),
            ("Console.enable", serde_json::json!({})),
        ];

        for (method, params) in commands {
            let msg = CdpTunnelMessage {
                id: 1,
                method: method.to_string(),
                params: params.clone(),
                is_request: true,
            };
            let json = serde_json::to_string(&msg).unwrap();
            let decoded: CdpTunnelMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded.method, method);
            assert_eq!(decoded.params, params);
        }
    }

    #[test]
    fn test_cdp_tunnel_message_large_payload() {
        let large_value = serde_json::json!({
            "data": "x".repeat(100_000),
        });
        let msg = CdpTunnelMessage {
            id: 999,
            method: "Runtime.evaluate".into(),
            params: large_value,
            is_request: true,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: CdpTunnelMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.params["data"].as_str().unwrap().len(), 100_000);
    }
}
