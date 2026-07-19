use std::time::Duration;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    middleware,
    routing::{get, post},
};
use bibi_work_backend::{
    configuration::{AgentRuntimeSettings, TelemetrySettings},
    features::agent_platform::runtime::{AgentRuntimeClient, DispatchRunRequest},
    telemetry::{http_trace_middleware, init_subscriber},
};
use secrecy::SecretBox;
use serde_json::json;
use tokio::{net::TcpListener, sync::mpsc};
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exports_http_span_and_preserves_incoming_trace_id() {
    let (export_tx, mut export_rx) = mpsc::channel::<(HeaderMap, Bytes)>(4);
    let collector_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let collector_address = collector_listener.local_addr().unwrap();
    let collector = Router::new().route(
        "/v1/traces",
        post(move |headers: HeaderMap, body: Bytes| {
            let export_tx = export_tx.clone();
            async move {
                export_tx.send((headers, body)).await.unwrap();
                StatusCode::OK
            }
        }),
    );
    let collector_task = tokio::spawn(async move {
        axum::serve(collector_listener, collector).await.unwrap();
    });

    let settings = TelemetrySettings {
        otlp_enabled: true,
        otlp_endpoint: Some(format!("http://{collector_address}")),
        service_name: "bibi-work-telemetry-test".to_owned(),
        trace_sample_ratio: 1.0,
        timeout_milliseconds: 2_000,
    };
    let telemetry_guard = init_subscriber(&settings).unwrap();

    let (runtime_tx, mut runtime_rx) = mpsc::channel::<HeaderMap>(1);
    let runtime_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let runtime_address = runtime_listener.local_addr().unwrap();
    let runtime = Router::new().route(
        "/internal/agent-runs",
        post(move |headers: HeaderMap| {
            let runtime_tx = runtime_tx.clone();
            async move {
                runtime_tx.send(headers).await.unwrap();
                StatusCode::OK
            }
        }),
    );
    let runtime_task = tokio::spawn(async move {
        axum::serve(runtime_listener, runtime).await.unwrap();
    });
    let runtime_client = AgentRuntimeClient::new(AgentRuntimeSettings {
        base_url: Some(format!("http://{runtime_address}")),
        shared_token: SecretBox::new("test-token".to_owned().into_boxed_str()),
        timeout_milliseconds: 2_000,
    })
    .unwrap();

    let app_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let app_address = app_listener.local_addr().unwrap();
    let app = Router::new()
        .route(
            "/dispatch",
            get(|State(client): State<AgentRuntimeClient>| async move {
                client
                    .dispatch_run(&DispatchRunRequest {
                        tenant_id: Uuid::new_v4(),
                        conversation_id: Uuid::new_v4(),
                        run_id: Uuid::new_v4(),
                        trace_id: "application-trace-id".to_owned(),
                        input: json!({"prompt": "trace contract"}),
                        run_config_snapshot: json!({}),
                    })
                    .await
                    .unwrap();
                StatusCode::NO_CONTENT
            }),
        )
        .layer(middleware::from_fn(http_trace_middleware))
        .with_state(runtime_client);
    let app_task = tokio::spawn(async move {
        axum::serve(app_listener, app).await.unwrap();
    });

    let trace_id = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32,
        0x10,
    ];
    let response = reqwest::Client::new()
        .get(format!("http://{app_address}/dispatch"))
        .header(
            "traceparent",
            "00-0123456789abcdeffedcba9876543210-0123456789abcdef-01",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let runtime_headers = tokio::time::timeout(Duration::from_secs(2), runtime_rx.recv())
        .await
        .expect("runtime did not receive dispatch")
        .expect("runtime trace header channel closed");
    assert_eq!(
        runtime_headers
            .get("traceparent")
            .unwrap()
            .to_str()
            .unwrap()
            .split('-')
            .nth(1),
        Some("0123456789abcdeffedcba9876543210")
    );

    app_task.abort();
    runtime_task.abort();
    drop(telemetry_guard);

    let (headers, body) = tokio::time::timeout(Duration::from_secs(5), export_rx.recv())
        .await
        .expect("OTLP collector did not receive an export")
        .expect("OTLP export channel closed");
    assert_eq!(
        headers.get("content-type").unwrap(),
        "application/x-protobuf"
    );
    assert!(
        body.windows(trace_id.len())
            .any(|window| window == trace_id),
        "exported protobuf did not contain the incoming W3C trace id"
    );

    collector_task.abort();
}
