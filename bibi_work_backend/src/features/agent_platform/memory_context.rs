use crate::features::agent_platform::models::{MemoryContextResponse, MemoryItemResponse};

pub const DEFAULT_MAX_MEMORY_CONTEXT_CHARS: usize = 1200;

const SENSITIVE_KEYS: &[&str] = &[
    "api_key",
    "api-key",
    "apikey",
    "token",
    "secret",
    "password",
    "authorization",
];

pub fn memory_context_from_item(
    memory: MemoryItemResponse,
    score: Option<f64>,
    source: &str,
    max_chars: usize,
) -> Option<MemoryContextResponse> {
    if memory.status != "approved" || memory.sensitivity == "secret" {
        return None;
    }

    let content = truncate_text(&redact_sensitive_text(&memory.content), max_chars);
    if content.trim().is_empty() {
        return None;
    }

    Some(MemoryContextResponse {
        memory_id: memory.id,
        layer: memory.layer,
        content,
        score,
        confidence: memory.confidence,
        visibility: memory.visibility,
        sensitivity: memory.sensitivity,
        source: source.to_string(),
        untrusted: true,
    })
}

pub fn redact_sensitive_text(input: &str) -> String {
    let redacted = redact_authorization_bearer(input);
    redact_key_values(&redacted)
}

pub fn truncate_text(input: &str, max_chars: usize) -> String {
    let total_chars = input.chars().count();
    if max_chars == 0 {
        return String::new();
    }
    if total_chars <= max_chars {
        return input.to_string();
    }

    let suffix = " [truncated]";
    if max_chars <= suffix.len() {
        return input.chars().take(max_chars).collect();
    }

    let kept: String = input
        .chars()
        .take(max_chars - suffix.len())
        .collect::<String>()
        .trim_end()
        .to_string();
    format!("{kept}{suffix}")
}

fn redact_authorization_bearer(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0_usize;

    while let Some(relative_idx) = lower[cursor..].find("authorization") {
        let key_start = cursor + relative_idx;
        let mut idx = key_start + "authorization".len();
        idx = skip_ascii_spaces(input, idx);
        if !matches_separator(input, idx) {
            cursor = idx;
            continue;
        }
        idx += 1;
        idx = skip_ascii_spaces(input, idx);
        if !lower[idx..].starts_with("bearer") {
            cursor = idx;
            continue;
        }
        let bearer_end = idx + "bearer".len();
        let token_start = skip_ascii_spaces(input, bearer_end);
        if token_start == bearer_end {
            cursor = bearer_end;
            continue;
        }
        let token_end = value_end(input, token_start);

        output.push_str(&input[cursor..token_start]);
        output.push_str("[REDACTED]");
        cursor = token_end;
    }

    output.push_str(&input[cursor..]);
    output
}

fn redact_key_values(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0_usize;

    while cursor < input.len() {
        let Some((key_start, key_len)) = find_next_sensitive_key(&lower, cursor) else {
            output.push_str(&input[cursor..]);
            break;
        };

        let mut idx = key_start + key_len;
        idx = skip_ascii_spaces(input, idx);
        if !matches_separator(input, idx) {
            output.push_str(&input[cursor..idx]);
            cursor = idx;
            continue;
        }

        let value_start = skip_ascii_spaces(input, idx + 1);
        let value_end = value_end(input, value_start);
        if &lower[key_start..key_start + key_len] == "authorization"
            && lower[value_start..].starts_with("bearer")
        {
            output.push_str(&input[cursor..value_end]);
            cursor = value_end;
            continue;
        }
        output.push_str(&input[cursor..value_start]);
        output.push_str("[REDACTED]");
        cursor = value_end;
    }

    output
}

fn find_next_sensitive_key(lower: &str, start: usize) -> Option<(usize, usize)> {
    SENSITIVE_KEYS
        .iter()
        .filter_map(|key| lower[start..].find(key).map(|idx| (start + idx, key.len())))
        .min_by_key(|(idx, _)| *idx)
}

fn skip_ascii_spaces(input: &str, mut idx: usize) -> usize {
    while idx < input.len() && input.as_bytes()[idx].is_ascii_whitespace() {
        idx += 1;
    }
    idx
}

fn matches_separator(input: &str, idx: usize) -> bool {
    idx < input.len() && matches!(input.as_bytes()[idx], b':' | b'=')
}

fn value_end(input: &str, mut idx: usize) -> usize {
    while idx < input.len() {
        let byte = input.as_bytes()[idx];
        if byte.is_ascii_whitespace() || matches!(byte, b',' | b';') {
            break;
        }
        idx += 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn memory(status: &str, sensitivity: &str, content: &str) -> MemoryItemResponse {
        MemoryItemResponse {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            user_id: Some(Uuid::new_v4()),
            agent_id: None,
            project_id: None,
            source_run_id: None,
            layer: "semantic".to_string(),
            content: content.to_string(),
            confidence: 0.7,
            status: status.to_string(),
            visibility: "private".to_string(),
            sensitivity: sensitivity.to_string(),
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn context_skips_unapproved_and_secret_memories() {
        assert!(
            memory_context_from_item(
                memory("candidate", "normal", "pending"),
                Some(0.9),
                "memory_vector_search",
                DEFAULT_MAX_MEMORY_CONTEXT_CHARS,
            )
            .is_none()
        );
        assert!(
            memory_context_from_item(
                memory("approved", "secret", "hidden"),
                Some(0.9),
                "memory_vector_search",
                DEFAULT_MAX_MEMORY_CONTEXT_CHARS,
            )
            .is_none()
        );
    }

    #[test]
    fn context_redacts_truncates_and_marks_untrusted() {
        let context = memory_context_from_item(
            memory(
                "approved",
                "normal",
                "sales token=plain-secret authorization: Bearer raw-secret",
            ),
            Some(0.93),
            "memory_vector_search",
            80,
        )
        .expect("context");

        assert_eq!(context.score, Some(0.93));
        assert!(context.untrusted);
        assert!(context.content.contains("token=[REDACTED]"));
        assert!(context.content.contains("Bearer [REDACTED]"));
        assert!(!context.content.contains("plain-secret"));
        assert!(!context.content.contains("raw-secret"));
    }

    #[test]
    fn truncate_text_handles_multibyte_text() {
        assert_eq!(truncate_text("销售额数据", 3), "销售额");
        assert!(
            truncate_text("销售额数据持续增长超过预期并需要截断", 16).ends_with(" [truncated]")
        );
    }
}
