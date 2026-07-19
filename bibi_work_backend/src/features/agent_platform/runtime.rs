use std::time::Duration;

use opentelemetry::{global, propagation::Injector};
use reqwest::{
    Client,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use secrecy::ExposeSecret;
use serde::Serialize;
use serde_json::Value;
use tracing::warn;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use uuid::Uuid;

use crate::{configuration::AgentRuntimeSettings, features::core::errors::AppError};

#[derive(Clone)]
pub struct AgentRuntimeClient {
    http: Client,
    base_url: Option<String>,
    shared_token: String,
}

#[derive(Debug, Serialize)]
pub struct DispatchRunRequest {
    pub tenant_id: Uuid,
    pub conversation_id: Uuid,
    pub run_id: Uuid,
    pub trace_id: String,
    pub input: Value,
    pub run_config_snapshot: Value,
}

#[derive(Debug, Serialize)]
pub struct ResumeRunRequest {
    pub tenant_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub approval_id: Uuid,
    pub trace_id: Option<String>,
    pub input: Value,
    pub run_config_snapshot: Value,
    pub thread_id: Option<String>,
    pub checkpoint_id: Option<String>,
    pub decision_payload: Value,
}

#[derive(Debug, Serialize)]
pub struct CancelRunRequest {
    pub tenant_id: Uuid,
    pub conversation_id: Uuid,
    pub trace_id: Option<String>,
    pub reason: String,
}

impl AgentRuntimeClient {
    pub fn new(settings: AgentRuntimeSettings) -> Result<Self, reqwest::Error> {
        let http = Client::builder()
            .timeout(Duration::from_millis(settings.timeout_milliseconds))
            .build()?;

        Ok(Self {
            http,
            base_url: settings.base_url.map(trim_trailing_slash),
            shared_token: settings.shared_token.expose_secret().to_string(),
        })
    }

    pub async fn dispatch_run(&self, payload: &DispatchRunRequest) -> Result<(), AppError> {
        let Some(base_url) = self.base_url.as_deref() else {
            warn!(
                "agent runtime is not configured; run {} remains queued",
                payload.run_id
            );
            return Ok(());
        };

        self.http
            .post(format!("{base_url}/internal/agent-runs"))
            .bearer_auth(&self.shared_token)
            .headers(current_trace_headers())
            .json(payload)
            .send()
            .await
            .map_err(|err| {
                warn!("failed to dispatch run {}: {}", payload.run_id, err);
                AppError::Unauthorized("agent runtime dispatch failed".to_string())
            })?
            .error_for_status()
            .map_err(|err| {
                warn!("agent runtime rejected run {}: {}", payload.run_id, err);
                AppError::Unauthorized("agent runtime dispatch failed".to_string())
            })?;

        Ok(())
    }

    pub async fn resume_run(
        &self,
        run_id: Uuid,
        payload: &ResumeRunRequest,
    ) -> Result<(), AppError> {
        let Some(base_url) = self.base_url.as_deref() else {
            warn!(
                "agent runtime is not configured; run {} cannot resume",
                run_id
            );
            return Err(AppError::Unauthorized(
                "agent runtime resume is not configured".to_string(),
            ));
        };

        self.http
            .post(format!("{base_url}/internal/agent-runs/{run_id}/resume"))
            .bearer_auth(&self.shared_token)
            .headers(current_trace_headers())
            .json(payload)
            .send()
            .await
            .map_err(|err| {
                warn!("failed to resume run {}: {}", run_id, err);
                AppError::Unauthorized("agent runtime resume failed".to_string())
            })?
            .error_for_status()
            .map_err(|err| {
                warn!("agent runtime rejected resume {}: {}", run_id, err);
                AppError::Unauthorized("agent runtime resume failed".to_string())
            })?;

        Ok(())
    }

    pub async fn cancel_run(
        &self,
        run_id: Uuid,
        payload: &CancelRunRequest,
    ) -> Result<(), AppError> {
        let Some(base_url) = self.base_url.as_deref() else {
            warn!(
                "agent runtime is not configured; run {} cannot receive cancel",
                run_id
            );
            return Ok(());
        };

        self.http
            .post(format!("{base_url}/internal/agent-runs/{run_id}/cancel"))
            .bearer_auth(&self.shared_token)
            .headers(current_trace_headers())
            .json(payload)
            .send()
            .await
            .map_err(|err| {
                warn!("failed to cancel runtime run {}: {}", run_id, err);
                AppError::Unauthorized("agent runtime cancel failed".to_string())
            })?
            .error_for_status()
            .map_err(|err| {
                warn!("agent runtime rejected cancel {}: {}", run_id, err);
                AppError::Unauthorized("agent runtime cancel failed".to_string())
            })?;

        Ok(())
    }
}

struct HeaderInjector<'a>(&'a mut HeaderMap);

impl Injector for HeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        let Ok(name) = HeaderName::from_bytes(key.as_bytes()) else {
            return;
        };
        let Ok(value) = HeaderValue::from_str(&value) else {
            return;
        };
        self.0.insert(name, value);
    }
}

fn current_trace_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    let context = tracing::Span::current().context();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut HeaderInjector(&mut headers));
    });
    headers
}

fn trim_trailing_slash(mut input: String) -> String {
    while input.ends_with('/') {
        input.pop();
    }
    input
}

#[cfg(test)]
mod tests {
    use secrecy::SecretBox;
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn resume_without_runtime_base_url_fails_closed() {
        let client = AgentRuntimeClient::new(AgentRuntimeSettings {
            base_url: None,
            shared_token: secret("test-token"),
            timeout_milliseconds: 1000,
        })
        .expect("runtime client");

        let err = client
            .resume_run(
                Uuid::new_v4(),
                &ResumeRunRequest {
                    tenant_id: Uuid::new_v4(),
                    conversation_id: Some(Uuid::new_v4()),
                    approval_id: Uuid::new_v4(),
                    trace_id: Some("trace".to_string()),
                    input: json!({}),
                    run_config_snapshot: json!({}),
                    thread_id: Some("thread".to_string()),
                    checkpoint_id: None,
                    decision_payload: json!({"decision": "approved"}),
                },
            )
            .await
            .expect_err("resume should fail without runtime base_url");

        assert!(err.to_string().contains("resume is not configured"));
    }

    fn secret(value: &str) -> SecretBox<str> {
        SecretBox::new(value.to_string().into_boxed_str())
    }
}
