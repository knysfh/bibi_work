use std::{collections::BTreeMap, time::Duration};

use reqwest::{Client, StatusCode};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::configuration::MemoryVectorSettings;

#[derive(Clone)]
pub struct MemoryVectorClient {
    http: Client,
    settings: MemoryVectorSettings,
}

#[derive(Debug, Clone)]
pub struct MemoryVectorSearchRequest {
    pub tenant_id: Uuid,
    pub user_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub layer: Option<String>,
    pub query: String,
    pub limit: usize,
    pub min_score: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct MemoryVectorIndexRequest {
    pub memory_id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub layer: String,
    pub content: String,
    pub content_hash: String,
    pub confidence: f64,
    pub status: String,
    pub visibility: String,
    pub sensitivity: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryVectorIndexResult {
    pub collection_name: String,
    pub point_id: String,
    pub vector_dimension: i32,
    pub vector_hash: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryVectorHit {
    pub memory_id: Uuid,
    pub score: f64,
}

#[derive(Debug, Serialize)]
struct EmbedRequest<'a> {
    inputs: &'a str,
}

impl MemoryVectorClient {
    pub fn new(settings: MemoryVectorSettings) -> Result<Self, reqwest::Error> {
        let http = Client::builder()
            .timeout(Duration::from_millis(settings.timeout_milliseconds))
            .build()?;

        Ok(Self { http, settings })
    }

    pub fn is_enabled(&self) -> bool {
        self.settings.enabled
            && self.settings.embedding_endpoint.is_some()
            && self.settings.qdrant_rest_url.is_some()
    }

    pub fn max_context_chars(&self) -> usize {
        self.settings.max_context_chars
    }

    pub fn worker_interval_milliseconds(&self) -> u64 {
        self.settings.worker_interval_milliseconds
    }

    pub fn worker_batch_size(&self) -> i64 {
        self.settings.worker_batch_size.max(1)
    }

    pub fn worker_max_attempts(&self) -> i32 {
        self.settings.worker_max_attempts.max(1)
    }

    pub fn collection_name(&self) -> &str {
        &self.settings.qdrant_collection
    }

    pub async fn index_memory(
        &self,
        request: MemoryVectorIndexRequest,
    ) -> Result<MemoryVectorIndexResult, String> {
        if !self.is_enabled() {
            return Err("memory vector indexing is disabled".to_string());
        }

        let vector = self.embed(&request.content).await?;
        let vector_dimension = i32::try_from(vector.len())
            .map_err(|_| "embedding vector dimension is too large".to_string())?;
        let vector_hash = vector_hash(&vector);
        self.ensure_qdrant_collection(vector_dimension).await?;
        self.upsert_qdrant_point(&request, &vector).await?;

        Ok(MemoryVectorIndexResult {
            collection_name: self.settings.qdrant_collection.clone(),
            point_id: request.memory_id.to_string(),
            vector_dimension,
            vector_hash,
        })
    }

    pub async fn delete_memory_point(&self, memory_id: Uuid) -> Result<(), String> {
        if !self.is_enabled() {
            return Err("memory vector indexing is disabled".to_string());
        }

        let rest_url = self.qdrant_rest_url()?;
        let url = format!(
            "{rest_url}/collections/{}/points/delete?wait=true",
            self.settings.qdrant_collection
        );
        let response = self
            .http
            .post(url)
            .json(&json!({ "points": [memory_id.to_string()] }))
            .send()
            .await
            .map_err(|err| format!("qdrant delete request failed: {err}"))?;

        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(format!(
                "qdrant rejected delete request: {}",
                response.status()
            ))
        }
    }

    pub async fn search_memory_ids(
        &self,
        request: MemoryVectorSearchRequest,
    ) -> Result<Vec<MemoryVectorHit>, String> {
        if !self.is_enabled() {
            return Err("memory vector search is disabled".to_string());
        }

        let embedding = self.embed(&request.query).await?;
        self.search_qdrant(&request, embedding).await
    }

    async fn embed(&self, text: &str) -> Result<Vec<f64>, String> {
        if text.trim().is_empty() {
            return Err("embedding query is empty".to_string());
        }
        let endpoint = self
            .settings
            .embedding_endpoint
            .as_deref()
            .ok_or_else(|| "embedding endpoint is not configured".to_string())?;

        let payload = self
            .http
            .post(endpoint)
            .json(&EmbedRequest { inputs: text })
            .send()
            .await
            .map_err(|err| format!("embedding request failed: {err}"))?
            .error_for_status()
            .map_err(|err| format!("embedding endpoint rejected request: {err}"))?
            .json::<Value>()
            .await
            .map_err(|err| format!("embedding response decode failed: {err}"))?;

        parse_embedding_response(&payload)
    }

    async fn search_qdrant(
        &self,
        request: &MemoryVectorSearchRequest,
        vector: Vec<f64>,
    ) -> Result<Vec<MemoryVectorHit>, String> {
        let rest_url = self.qdrant_rest_url()?;
        let url = format!(
            "{rest_url}/collections/{}/points/search",
            self.settings.qdrant_collection
        );

        let mut body = json!({
            "vector": vector,
            "limit": request.limit,
            "with_payload": true,
            "filter": qdrant_filter(request),
        });
        if let Some(min_score) = request.min_score {
            body["score_threshold"] = json!(min_score);
        }

        let payload = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|err| format!("qdrant search request failed: {err}"))?
            .error_for_status()
            .map_err(|err| format!("qdrant rejected search request: {err}"))?
            .json::<Value>()
            .await
            .map_err(|err| format!("qdrant response decode failed: {err}"))?;

        parse_qdrant_hits(&payload)
    }

    async fn ensure_qdrant_collection(&self, vector_dimension: i32) -> Result<(), String> {
        let rest_url = self.qdrant_rest_url()?;
        let collection_url = format!("{rest_url}/collections/{}", self.settings.qdrant_collection);

        let response = self
            .http
            .get(&collection_url)
            .send()
            .await
            .map_err(|err| format!("qdrant collection check failed: {err}"))?;
        if response.status().is_success() {
            return Ok(());
        }
        if response.status() != StatusCode::NOT_FOUND {
            return Err(format!(
                "qdrant collection check failed with status {}",
                response.status()
            ));
        }

        let response = self
            .http
            .put(collection_url)
            .json(&qdrant_collection_body(vector_dimension))
            .send()
            .await
            .map_err(|err| format!("qdrant collection create failed: {err}"))?;

        if response.status().is_success() || response.status() == StatusCode::CONFLICT {
            Ok(())
        } else {
            Err(format!(
                "qdrant rejected collection create: {}",
                response.status()
            ))
        }
    }

    async fn upsert_qdrant_point(
        &self,
        request: &MemoryVectorIndexRequest,
        vector: &[f64],
    ) -> Result<(), String> {
        let rest_url = self.qdrant_rest_url()?;
        let url = format!(
            "{rest_url}/collections/{}/points?wait=true",
            self.settings.qdrant_collection
        );
        let response = self
            .http
            .put(url)
            .json(&qdrant_upsert_body(request, vector))
            .send()
            .await
            .map_err(|err| format!("qdrant upsert request failed: {err}"))?
            .error_for_status()
            .map_err(|err| format!("qdrant rejected upsert request: {err}"))?;
        drop(response);
        Ok(())
    }

    fn qdrant_rest_url(&self) -> Result<&str, String> {
        self.settings
            .qdrant_rest_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(|url| url.trim_end_matches('/'))
            .ok_or_else(|| "qdrant rest url is not configured".to_string())
    }
}

fn qdrant_filter(request: &MemoryVectorSearchRequest) -> Value {
    let mut must = vec![
        qdrant_match("tenant_id", request.tenant_id.to_string()),
        qdrant_match("status", "approved"),
    ];
    if let Some(user_id) = request.user_id {
        must.push(qdrant_match("user_id", user_id.to_string()));
    }
    if let Some(agent_id) = request.agent_id {
        must.push(qdrant_match("agent_id", agent_id.to_string()));
    }
    if let Some(project_id) = request.project_id {
        must.push(qdrant_match("project_id", project_id.to_string()));
    }
    if let Some(layer) = request.layer.as_deref().filter(|layer| !layer.is_empty()) {
        must.push(qdrant_match("layer", layer));
    }

    json!({ "must": must })
}

fn qdrant_match(key: &str, value: impl ToString) -> Value {
    json!({ "key": key, "match": { "value": value.to_string() } })
}

pub fn qdrant_collection_body(vector_dimension: i32) -> Value {
    json!({
        "vectors": {
            "size": vector_dimension,
            "distance": "Cosine"
        }
    })
}

pub fn qdrant_upsert_body(request: &MemoryVectorIndexRequest, vector: &[f64]) -> Value {
    json!({
        "points": [
            {
                "id": request.memory_id.to_string(),
                "vector": vector,
                "payload": qdrant_point_payload(request)
            }
        ]
    })
}

fn qdrant_point_payload(request: &MemoryVectorIndexRequest) -> Value {
    let mut payload = Map::new();
    payload.insert(
        "memory_id".to_string(),
        json!(request.memory_id.to_string()),
    );
    payload.insert(
        "tenant_id".to_string(),
        json!(request.tenant_id.to_string()),
    );
    insert_optional_uuid(&mut payload, "user_id", request.user_id);
    insert_optional_uuid(&mut payload, "agent_id", request.agent_id);
    insert_optional_uuid(&mut payload, "project_id", request.project_id);
    payload.insert("layer".to_string(), json!(request.layer));
    payload.insert("content_hash".to_string(), json!(request.content_hash));
    payload.insert("confidence".to_string(), json!(request.confidence));
    payload.insert("status".to_string(), json!(request.status));
    payload.insert("visibility".to_string(), json!(request.visibility));
    payload.insert("sensitivity".to_string(), json!(request.sensitivity));
    Value::Object(payload)
}

fn insert_optional_uuid(payload: &mut Map<String, Value>, key: &str, value: Option<Uuid>) {
    if let Some(value) = value {
        payload.insert(key.to_string(), json!(value.to_string()));
    }
}

pub fn parse_embedding_response(payload: &Value) -> Result<Vec<f64>, String> {
    match payload {
        Value::Object(map) => {
            if let Some(value) = map.get("embedding") {
                return numeric_vector(value);
            }
            if let Some(value) = map.get("embeddings") {
                return first_numeric_vector(value);
            }
            if let Some(Value::Array(data)) = map.get("data") {
                if let Some(Value::Object(first)) = data.first()
                    && let Some(value) = first.get("embedding")
                {
                    return numeric_vector(value);
                }
                return first_numeric_vector(&Value::Array(data.clone()));
            }
        }
        Value::Array(items) => {
            if items.iter().all(Value::is_number) {
                return numeric_vector(payload);
            }
            return first_numeric_vector(payload);
        }
        _ => {}
    }

    Err("embedding response did not contain a numeric vector".to_string())
}

fn first_numeric_vector(payload: &Value) -> Result<Vec<f64>, String> {
    match payload {
        Value::Array(items) if !items.is_empty() => numeric_vector(&items[0]),
        _ => Err("embedding response did not contain a numeric vector".to_string()),
    }
}

fn numeric_vector(payload: &Value) -> Result<Vec<f64>, String> {
    let Value::Array(items) = payload else {
        return Err("embedding vector must be an array".to_string());
    };
    if items.is_empty() {
        return Err("embedding vector must not be empty".to_string());
    }

    items
        .iter()
        .map(|value| {
            value
                .as_f64()
                .ok_or_else(|| "embedding vector must contain only numbers".to_string())
        })
        .collect()
}

fn parse_qdrant_hits(payload: &Value) -> Result<Vec<MemoryVectorHit>, String> {
    let result = payload
        .get("result")
        .and_then(Value::as_array)
        .ok_or_else(|| "qdrant response did not contain result array".to_string())?;

    let mut hits = Vec::new();
    for point in result {
        let Some(memory_id) = qdrant_point_memory_id(point) else {
            continue;
        };
        let score = point.get("score").and_then(Value::as_f64).unwrap_or(0.0);
        hits.push(MemoryVectorHit { memory_id, score });
    }

    Ok(hits)
}

fn qdrant_point_memory_id(point: &Value) -> Option<Uuid> {
    point
        .get("payload")
        .and_then(|payload| payload.get("memory_id"))
        .and_then(Value::as_str)
        .or_else(|| point.get("id").and_then(Value::as_str))
        .and_then(|id| Uuid::parse_str(id).ok())
}

pub fn score_by_memory_id(hits: &[MemoryVectorHit]) -> BTreeMap<Uuid, f64> {
    hits.iter()
        .map(|hit| (hit.memory_id, hit.score))
        .collect::<BTreeMap<_, _>>()
}

fn vector_hash(vector: &[f64]) -> String {
    let mut hasher = Sha256::new();
    for value in vector {
        hasher.update(value.to_le_bytes());
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_embedding_response_accepts_supported_shapes() {
        assert_eq!(
            parse_embedding_response(&json!([[0.1, 0.2]])).expect("nested"),
            vec![0.1, 0.2]
        );
        assert_eq!(
            parse_embedding_response(&json!({"embedding": [1, 2]})).expect("embedding"),
            vec![1.0, 2.0]
        );
        assert_eq!(
            parse_embedding_response(&json!({"data": [{"embedding": [3, 4]}]})).expect("data"),
            vec![3.0, 4.0]
        );
    }

    #[test]
    fn qdrant_filter_includes_tenant_status_and_scope() {
        let request = MemoryVectorSearchRequest {
            tenant_id: Uuid::nil(),
            user_id: Some(Uuid::nil()),
            agent_id: None,
            project_id: None,
            layer: Some("semantic".to_string()),
            query: "sales".to_string(),
            limit: 3,
            min_score: Some(0.2),
        };

        assert_eq!(
            qdrant_filter(&request),
            json!({
                "must": [
                    {"key": "tenant_id", "match": {"value": Uuid::nil().to_string()}},
                    {"key": "status", "match": {"value": "approved"}},
                    {"key": "user_id", "match": {"value": Uuid::nil().to_string()}},
                    {"key": "layer", "match": {"value": "semantic"}}
                ]
            })
        );
    }

    #[test]
    fn qdrant_collection_body_uses_cosine_distance() {
        assert_eq!(
            qdrant_collection_body(768),
            json!({"vectors": {"size": 768, "distance": "Cosine"}})
        );
    }

    #[test]
    fn qdrant_upsert_body_omits_raw_memory_content() {
        let request = MemoryVectorIndexRequest {
            memory_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            user_id: Some(Uuid::nil()),
            agent_id: None,
            project_id: None,
            layer: "semantic".to_string(),
            content: "raw memory content with token=secret".to_string(),
            content_hash: "content-hash".to_string(),
            confidence: 0.8,
            status: "approved".to_string(),
            visibility: "private".to_string(),
            sensitivity: "normal".to_string(),
        };

        let body = qdrant_upsert_body(&request, &[0.1, 0.2]);
        let payload = &body["points"][0]["payload"];
        assert_eq!(payload["memory_id"], json!(Uuid::nil().to_string()));
        assert_eq!(payload["tenant_id"], json!(Uuid::nil().to_string()));
        assert_eq!(payload["content_hash"], json!("content-hash"));
        assert!(payload.get("content").is_none());
    }

    #[test]
    fn parse_qdrant_hits_prefers_payload_memory_id() {
        let memory_id = Uuid::new_v4();
        let other_id = Uuid::new_v4();
        let hits = parse_qdrant_hits(&json!({
            "result": [
                {
                    "id": other_id.to_string(),
                    "score": 0.91,
                    "payload": {"memory_id": memory_id.to_string()}
                }
            ]
        }))
        .expect("hits");

        assert_eq!(
            hits,
            vec![MemoryVectorHit {
                memory_id,
                score: 0.91
            }]
        );
    }
}
