use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::features::core::errors::AppError;

pub fn build_node_input_envelope(
    workflow_run_id: Uuid,
    workflow_version_id: Option<Uuid>,
    workflow_input: &Value,
    node_key: &str,
    node: &Value,
    upstream_outputs: Map<String, Value>,
) -> Result<Value, AppError> {
    let context = input_mapping_context(
        workflow_run_id,
        workflow_version_id,
        workflow_input,
        node_key,
        node,
        &upstream_outputs,
    );
    let mapped_input = node
        .get("input_mapping")
        .map(|mapping| apply_mapping("input_mapping", mapping, &context))
        .transpose()?;
    let upstream_nodes = context.get("nodes").cloned().unwrap_or_else(|| json!({}));

    let mut envelope = json!({
        "workflow_run_id": workflow_run_id,
        "workflow_version_id": workflow_version_id,
        "workflow_input": workflow_input,
        "node_key": node_key,
        "node": node,
        "nodes": upstream_nodes,
        "upstream_outputs": upstream_outputs
    });

    if let Some(mapped_input) = mapped_input {
        envelope
            .as_object_mut()
            .expect("node input envelope is an object")
            .insert("node_input".to_string(), mapped_input);
    }

    Ok(envelope)
}

pub fn map_terminal_output(
    node_run_input: &Value,
    event_payload: &Value,
) -> Result<Value, AppError> {
    let Some(node) = node_run_input.get("node") else {
        return Ok(event_payload.clone());
    };
    let Some(output_mapping) = node.get("output_mapping") else {
        return Ok(event_payload.clone());
    };

    let context = output_mapping_context(node_run_input, event_payload);
    apply_mapping("output_mapping", output_mapping, &context)
}

pub fn validate_node_mappings(node: &Value) -> Result<(), AppError> {
    let empty_context = json!({});
    if let Some(mapping) = node.get("input_mapping") {
        validate_mapping("input_mapping", mapping, &empty_context)?;
    }
    if let Some(mapping) = node.get("output_mapping") {
        validate_mapping("output_mapping", mapping, &empty_context)?;
    }
    Ok(())
}

fn validate_mapping(field_name: &str, mapping: &Value, context: &Value) -> Result<(), AppError> {
    match mapping {
        Value::Object(_) | Value::Array(_) => {
            apply_mapping(field_name, mapping, context).map(|_| ())
        }
        _ => Err(AppError::InvalidInput(format!(
            "workflow {field_name} must be a JSON object or array"
        ))),
    }
}

fn input_mapping_context(
    workflow_run_id: Uuid,
    workflow_version_id: Option<Uuid>,
    workflow_input: &Value,
    node_key: &str,
    node: &Value,
    upstream_outputs: &Map<String, Value>,
) -> Value {
    let mut upstream_nodes = Map::new();
    for (key, output) in upstream_outputs {
        upstream_nodes.insert(key.clone(), json!({ "output": output }));
    }

    json!({
        "workflow": {
            "run_id": workflow_run_id,
            "version_id": workflow_version_id,
            "input": workflow_input
        },
        "node": {
            "key": node_key,
            "definition": node
        },
        "nodes": upstream_nodes,
        "upstream_outputs": upstream_outputs
    })
}

fn output_mapping_context(node_run_input: &Value, event_payload: &Value) -> Value {
    let final_summary = event_payload
        .get("final_summary")
        .or_else(|| event_payload.get("summary"))
        .or_else(|| event_payload.get("message"))
        .cloned()
        .unwrap_or(Value::Null);

    json!({
        "agent": {
            "final": event_payload,
            "final_summary": final_summary
        },
        "event": event_payload,
        "run": event_payload,
        "workflow": {
            "run_id": node_run_input.get("workflow_run_id").cloned().unwrap_or(Value::Null),
            "version_id": node_run_input.get("workflow_version_id").cloned().unwrap_or(Value::Null),
            "input": node_run_input.get("workflow_input").cloned().unwrap_or(Value::Null)
        },
        "node": node_run_input.get("node").cloned().unwrap_or(Value::Null),
        "nodes": node_run_input.get("nodes").cloned().unwrap_or_else(|| json!({})),
        "upstream_outputs": node_run_input
            .get("upstream_outputs")
            .cloned()
            .unwrap_or_else(|| json!({}))
    })
}

fn apply_mapping(field_name: &str, mapping: &Value, context: &Value) -> Result<Value, AppError> {
    match mapping {
        Value::String(value) if value.starts_with('$') => {
            select_json_path(field_name, value, context)
        }
        Value::Array(values) => values
            .iter()
            .map(|value| apply_mapping(field_name, value, context))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(object) => object
            .iter()
            .map(|(key, value)| Ok((key.clone(), apply_mapping(field_name, value, context)?)))
            .collect::<Result<Map<String, Value>, AppError>>()
            .map(Value::Object),
        Value::String(_) => Ok(mapping.clone()),
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(mapping.clone()),
    }
}

fn select_json_path(
    field_name: &str,
    expression: &str,
    context: &Value,
) -> Result<Value, AppError> {
    let tokens = parse_json_path(field_name, expression)?;
    let mut current = context;
    for token in tokens {
        match token {
            PathToken::Field(field) => {
                let Some(next) = current.get(&field) else {
                    return Ok(Value::Null);
                };
                current = next;
            }
            PathToken::Index(index) => {
                let Some(next) = current.as_array().and_then(|values| values.get(index)) else {
                    return Ok(Value::Null);
                };
                current = next;
            }
        }
    }
    Ok(current.clone())
}

#[derive(Debug, PartialEq, Eq)]
enum PathToken {
    Field(String),
    Index(usize),
}

fn parse_json_path(field_name: &str, expression: &str) -> Result<Vec<PathToken>, AppError> {
    if expression == "$" {
        return Ok(Vec::new());
    }
    if !expression.starts_with("$.") && !expression.starts_with("$[") {
        return Err(AppError::InvalidInput(format!(
            "workflow {field_name} selector must start with '$.' or '$['"
        )));
    }

    let bytes = expression.as_bytes();
    let mut idx = 1;
    let mut tokens = Vec::new();
    while idx < bytes.len() {
        match bytes[idx] {
            b'.' => {
                idx += 1;
                let start = idx;
                while idx < bytes.len()
                    && matches!(bytes[idx], b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b':')
                {
                    idx += 1;
                }
                if start == idx {
                    return Err(AppError::InvalidInput(format!(
                        "workflow {field_name} selector has an empty field segment"
                    )));
                }
                tokens.push(PathToken::Field(expression[start..idx].to_string()));
            }
            b'[' => {
                idx += 1;
                let start = idx;
                while idx < bytes.len() && bytes[idx].is_ascii_digit() {
                    idx += 1;
                }
                if start == idx || idx >= bytes.len() || bytes[idx] != b']' {
                    return Err(AppError::InvalidInput(format!(
                        "workflow {field_name} selector only supports numeric array indexes"
                    )));
                }
                let index = expression[start..idx].parse::<usize>().map_err(|_| {
                    AppError::InvalidInput(format!(
                        "workflow {field_name} selector array index is too large"
                    ))
                })?;
                tokens.push(PathToken::Index(index));
                idx += 1;
            }
            _ => {
                return Err(AppError::InvalidInput(format!(
                    "workflow {field_name} selector contains an unsupported token"
                )));
            }
        }
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn build_node_input_envelope_applies_recursive_input_mapping() {
        let workflow_run_id = Uuid::new_v4();
        let workflow_version_id = Some(Uuid::new_v4());
        let node = json!({
            "node_key": "review",
            "input_mapping": {
                "prompt": "$.workflow.input.prompt",
                "summary": "$.nodes.prepare.output.summary",
                "first_artifact": "$.nodes.prepare.output.artifacts[0].path",
                "literal": "keep"
            }
        });
        let mut upstream_outputs = Map::new();
        upstream_outputs.insert(
            "prepare".to_string(),
            json!({
                "summary": "prepared",
                "artifacts": [{"path": "/workspace/report.md"}]
            }),
        );

        let envelope = build_node_input_envelope(
            workflow_run_id,
            workflow_version_id,
            &json!({"prompt": "review this"}),
            "review",
            &node,
            upstream_outputs,
        )
        .unwrap();

        assert_eq!(
            envelope.get("node_input"),
            Some(&json!({
                "prompt": "review this",
                "summary": "prepared",
                "first_artifact": "/workspace/report.md",
                "literal": "keep"
            }))
        );
        assert_eq!(
            envelope.pointer("/upstream_outputs/prepare/summary"),
            Some(&json!("prepared"))
        );
        assert_eq!(
            envelope.pointer("/nodes/prepare/output/summary"),
            Some(&json!("prepared"))
        );
    }

    #[test]
    fn map_terminal_output_applies_output_mapping() {
        let node_run_input = json!({
            "workflow_input": {"topic": "sales"},
            "node": {
                "output_mapping": {
                    "summary": "$.agent.final_summary",
                    "artifact": "$.run.artifacts[0]",
                    "source_topic": "$.workflow.input.topic"
                }
            }
        });
        let output = map_terminal_output(
            &node_run_input,
            &json!({
                "summary": "done",
                "artifacts": [{"path": "/workspace/out.md"}]
            }),
        )
        .unwrap();

        assert_eq!(
            output,
            json!({
                "summary": "done",
                "artifact": {"path": "/workspace/out.md"},
                "source_topic": "sales"
            })
        );
    }

    #[test]
    fn selector_missing_path_returns_null() {
        let output = apply_mapping(
            "input_mapping",
            &json!({"missing": "$.workflow.input.none"}),
            &json!({"workflow": {"input": {}}}),
        )
        .unwrap();

        assert_eq!(output, json!({"missing": null}));
    }

    #[test]
    fn validate_node_mappings_rejects_unsupported_selector() {
        let node = json!({
            "input_mapping": {
                "bad": "workflow.input"
            }
        });

        assert!(validate_node_mappings(&node).is_ok());

        let node = json!({
            "input_mapping": {
                "bad": "$..workflow"
            }
        });

        assert!(validate_node_mappings(&node).is_err());
    }
}
