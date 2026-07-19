use std::collections::{HashMap, HashSet};

use serde_json::Value;
use uuid::Uuid;

use crate::features::core::errors::AppError;

use super::workflow_mapping;

pub const MAX_WORKFLOW_NODES: usize = 500;
pub const MAX_WORKFLOW_EDGES: usize = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeExecutionPolicy {
    pub max_attempts: i32,
    pub backoff_sec: i32,
    pub timeout_sec: Option<i32>,
}

impl Default for NodeExecutionPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 1,
            backoff_sec: 0,
            timeout_sec: None,
        }
    }
}

pub fn validate(plan: &Value) -> Result<(), AppError> {
    let nodes = nodes(plan)?;
    let edges = edges(plan)?;
    concurrency_limit(plan)?;
    if nodes.is_empty() {
        return Err(AppError::InvalidInput(
            "compiled workflow must contain at least one node".to_string(),
        ));
    }
    if nodes.len() > MAX_WORKFLOW_NODES {
        return Err(AppError::InvalidInput(format!(
            "compiled workflow may contain at most {MAX_WORKFLOW_NODES} nodes"
        )));
    }
    if edges.len() > MAX_WORKFLOW_EDGES {
        return Err(AppError::InvalidInput(format!(
            "compiled workflow may contain at most {MAX_WORKFLOW_EDGES} edges"
        )));
    }

    let mut node_keys = HashSet::new();
    for (node_key, node) in &nodes {
        if !node_keys.insert(node_key.clone()) {
            return Err(AppError::InvalidInput(
                "each workflow node must have a unique key".to_string(),
            ));
        }
        if let Some(node_type) = node.get("node_type").and_then(Value::as_str)
            && node_type != "agent_task"
        {
            return Err(AppError::InvalidInput(
                "v1 workflow nodes only support agent_task".to_string(),
            ));
        }
        node_agent_version_id(node)?;
        node_execution_policy(node)?;
        workflow_mapping::validate_node_mappings(node)?;
    }

    let mut indegree = HashMap::<String, usize>::new();
    let mut adjacency = HashMap::<String, Vec<String>>::new();
    for node_key in &node_keys {
        indegree.insert(node_key.clone(), 0);
    }
    for (from, to) in edges {
        if from == to {
            return Err(AppError::InvalidInput(
                "workflow edge may not reference the same node".to_string(),
            ));
        }
        if !node_keys.contains(&from) || !node_keys.contains(&to) {
            return Err(AppError::InvalidInput(
                "workflow edge references an unknown node".to_string(),
            ));
        }
        adjacency.entry(from).or_default().push(to.clone());
        *indegree.entry(to).or_default() += 1;
    }

    let mut ready = indegree
        .iter()
        .filter_map(|(key, degree)| (*degree == 0).then_some(key.clone()))
        .collect::<Vec<_>>();
    let mut visited = 0_usize;
    while let Some(key) = ready.pop() {
        visited += 1;
        if let Some(children) = adjacency.get(&key) {
            for child in children {
                let degree = indegree.get_mut(child).ok_or_else(|| {
                    AppError::InvalidInput("workflow edge references an unknown node".to_string())
                })?;
                *degree -= 1;
                if *degree == 0 {
                    ready.push(child.clone());
                }
            }
        }
    }
    if visited != node_keys.len() {
        return Err(AppError::InvalidInput(
            "workflow DAG contains a cycle".to_string(),
        ));
    }

    Ok(())
}

pub fn concurrency_limit(plan: &Value) -> Result<Option<i64>, AppError> {
    let value = plan
        .get("concurrency_limit")
        .or_else(|| plan.get("max_concurrency"))
        .or_else(|| {
            plan.get("execution_policy")
                .and_then(|policy| policy.get("concurrency_limit"))
        })
        .or_else(|| {
            plan.get("execution_policy")
                .and_then(|policy| policy.get("max_concurrency"))
        });

    let Some(value) = value else {
        return Ok(None);
    };
    let Some(limit) = value.as_i64() else {
        return Err(AppError::InvalidInput(
            "workflow concurrency limit must be an integer".to_string(),
        ));
    };
    if !(1..=100).contains(&limit) {
        return Err(AppError::InvalidInput(
            "workflow concurrency limit must be between 1 and 100".to_string(),
        ));
    }

    Ok(Some(limit))
}

pub fn nodes(plan: &Value) -> Result<Vec<(String, Value)>, AppError> {
    let nodes = plan.get("nodes").and_then(Value::as_array).ok_or_else(|| {
        AppError::InvalidInput("compiled_plan.nodes must be an array".to_string())
    })?;
    nodes
        .iter()
        .map(|node| {
            let key = node_key(node)
                .ok_or_else(|| {
                    AppError::InvalidInput("workflow node requires key or node_key".to_string())
                })?
                .to_string();
            Ok((key, node.clone()))
        })
        .collect()
}

pub fn node_map(plan: &Value) -> Result<HashMap<String, Value>, AppError> {
    Ok(nodes(plan)?.into_iter().collect())
}

pub fn edges(plan: &Value) -> Result<Vec<(String, String)>, AppError> {
    let edges = plan.get("edges").and_then(Value::as_array).ok_or_else(|| {
        AppError::InvalidInput("compiled_plan.edges must be an array".to_string())
    })?;
    edges
        .iter()
        .map(|edge| {
            let from = edge
                .get("from")
                .or_else(|| edge.get("source"))
                .and_then(Value::as_str)
                .ok_or_else(|| AppError::InvalidInput("workflow edge requires from".to_string()))?;
            let to = edge
                .get("to")
                .or_else(|| edge.get("target"))
                .and_then(Value::as_str)
                .ok_or_else(|| AppError::InvalidInput("workflow edge requires to".to_string()))?;
            Ok((from.to_string(), to.to_string()))
        })
        .collect()
}

pub fn node_agent_version_id(node: &Value) -> Result<Uuid, AppError> {
    let agent_version_id = node
        .get("agent_version_id")
        .or_else(|| {
            node.get("agent")
                .and_then(|agent| agent.get("agent_version_id"))
        })
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AppError::InvalidInput("workflow node requires agent_version_id".to_string())
        })?;
    Uuid::parse_str(agent_version_id)
        .map_err(|_| AppError::InvalidInput("agent_version_id must be a uuid".to_string()))
}

pub fn node_execution_policy(node: &Value) -> Result<NodeExecutionPolicy, AppError> {
    let retry_policy = node.get("retry_policy");
    let max_attempts = retry_policy
        .and_then(|policy| policy.get("max_attempts"))
        .and_then(Value::as_i64)
        .unwrap_or(1);
    if !(1..=10).contains(&max_attempts) {
        return Err(AppError::InvalidInput(
            "workflow retry_policy.max_attempts must be between 1 and 10".to_string(),
        ));
    }

    let backoff_sec = retry_policy
        .and_then(|policy| policy.get("backoff_sec"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if !(0..=86_400).contains(&backoff_sec) {
        return Err(AppError::InvalidInput(
            "workflow retry_policy.backoff_sec must be between 0 and 86400".to_string(),
        ));
    }

    let timeout_sec = node
        .get("timeout_sec")
        .and_then(Value::as_i64)
        .map(|value| {
            if !(1..=86_400).contains(&value) {
                Err(AppError::InvalidInput(
                    "workflow timeout_sec must be between 1 and 86400".to_string(),
                ))
            } else {
                i32::try_from(value).map(Some).map_err(|_| {
                    AppError::InvalidInput("workflow timeout_sec is too large".to_string())
                })
            }
        })
        .transpose()?
        .flatten();

    Ok(NodeExecutionPolicy {
        max_attempts: i32::try_from(max_attempts).map_err(|_| {
            AppError::InvalidInput("workflow retry_policy.max_attempts is too large".to_string())
        })?,
        backoff_sec: i32::try_from(backoff_sec).map_err(|_| {
            AppError::InvalidInput("workflow retry_policy.backoff_sec is too large".to_string())
        })?,
        timeout_sec,
    })
}

fn node_key(node: &Value) -> Option<&str> {
    node.get("node_key")
        .or_else(|| node.get("key"))
        .and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn validate_rejects_unbounded_retry_policy() {
        let plan = json!({
            "nodes": [{
                "node_key": "a",
                "node_type": "agent_task",
                "agent_version_id": Uuid::new_v4().to_string(),
                "retry_policy": {"max_attempts": 11}
            }],
            "edges": []
        });

        assert!(validate(&plan).is_err());
    }

    #[test]
    fn validate_rejects_invalid_concurrency_limit() {
        let plan = json!({
            "concurrency_limit": 0,
            "nodes": [{
                "node_key": "a",
                "node_type": "agent_task",
                "agent_version_id": Uuid::new_v4().to_string()
            }],
            "edges": []
        });

        assert!(validate(&plan).is_err());
    }

    #[test]
    fn concurrency_limit_accepts_supported_fields() {
        let direct = json!({ "concurrency_limit": 3 });
        let nested = json!({ "execution_policy": { "max_concurrency": 2 } });

        assert_eq!(concurrency_limit(&direct).unwrap(), Some(3));
        assert_eq!(concurrency_limit(&nested).unwrap(), Some(2));
        assert_eq!(concurrency_limit(&json!({})).unwrap(), None);
    }

    #[test]
    fn node_execution_policy_uses_safe_defaults() {
        let node = json!({
            "node_key": "a",
            "agent_version_id": Uuid::new_v4().to_string()
        });

        assert_eq!(
            node_execution_policy(&node).unwrap(),
            NodeExecutionPolicy::default()
        );
    }

    #[test]
    fn node_execution_policy_parses_retry_and_timeout() {
        let node = json!({
            "node_key": "a",
            "agent_version_id": Uuid::new_v4().to_string(),
            "retry_policy": {"max_attempts": 3, "backoff_sec": 15},
            "timeout_sec": 120
        });

        assert_eq!(
            node_execution_policy(&node).unwrap(),
            NodeExecutionPolicy {
                max_attempts: 3,
                backoff_sec: 15,
                timeout_sec: Some(120)
            }
        );
    }

    #[test]
    fn validate_rejects_invalid_mapping_selector() {
        let plan = json!({
            "nodes": [{
                "node_key": "a",
                "agent_version_id": Uuid::new_v4().to_string(),
                "input_mapping": {
                    "bad": "$..workflow"
                }
            }],
            "edges": []
        });

        assert!(validate(&plan).is_err());
    }

    #[test]
    fn validate_accepts_maximum_node_count() {
        let agent_version_id = Uuid::new_v4().to_string();
        let nodes = (0..MAX_WORKFLOW_NODES)
            .map(|index| {
                json!({
                    "node_key": format!("node-{index}"),
                    "agent_version_id": agent_version_id
                })
            })
            .collect::<Vec<_>>();
        let plan = json!({"nodes": nodes, "edges": []});

        validate(&plan).unwrap();
    }

    #[test]
    fn validate_rejects_workflow_over_node_limit() {
        let agent_version_id = Uuid::new_v4().to_string();
        let nodes = (0..=MAX_WORKFLOW_NODES)
            .map(|index| {
                json!({
                    "node_key": format!("node-{index}"),
                    "agent_version_id": agent_version_id
                })
            })
            .collect::<Vec<_>>();
        let plan = json!({"nodes": nodes, "edges": []});

        let error = validate(&plan).unwrap_err().to_string();
        assert!(error.contains("at most 500 nodes"));
    }

    #[test]
    fn validate_accepts_maximum_edge_count() {
        let agent_version_id = Uuid::new_v4().to_string();
        let edges = (0..MAX_WORKFLOW_EDGES)
            .map(|_| json!({"from": "a", "to": "b"}))
            .collect::<Vec<_>>();
        let plan = json!({
            "nodes": [
                {"node_key": "a", "agent_version_id": agent_version_id},
                {"node_key": "b", "agent_version_id": agent_version_id}
            ],
            "edges": edges
        });

        validate(&plan).unwrap();
    }

    #[test]
    fn validate_rejects_workflow_over_edge_limit() {
        let agent_version_id = Uuid::new_v4().to_string();
        let edges = (0..=MAX_WORKFLOW_EDGES)
            .map(|_| json!({"from": "a", "to": "b"}))
            .collect::<Vec<_>>();
        let plan = json!({
            "nodes": [
                {"node_key": "a", "agent_version_id": agent_version_id},
                {"node_key": "b", "agent_version_id": agent_version_id}
            ],
            "edges": edges
        });

        let error = validate(&plan).unwrap_err().to_string();
        assert!(error.contains("at most 5000 edges"));
    }
}
