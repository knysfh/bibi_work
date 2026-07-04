use std::collections::HashSet;

use serde_json::Value;
use sqlx::{PgPool, Row};
use tracing::warn;

use super::models::{
    AuthzBatchCheckRequest, AuthzBatchCheckResponse, AuthzCheckRequest, AuthzDecision,
};

const LOCAL_POLICY_VERSION: &str = "local-policy-v1";
const LOCAL_POLICY_ERROR: &str = "local-policy-error";

#[derive(Clone)]
pub struct ResourceAuthzService {
    pool: PgPool,
}

#[derive(Debug)]
struct ActorAuthorizationContext {
    roles: HashSet<String>,
    tenant_membership_role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BindingEffect {
    Allow,
    Deny,
    Review,
}

impl ResourceAuthzService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn check(&self, request: &AuthzCheckRequest) -> AuthzDecision {
        match self.check_local(request).await {
            Ok(decision) => decision,
            Err(err) => {
                warn!("local authz check failed closed: {}", err);
                AuthzDecision::deny(LOCAL_POLICY_ERROR, "local_authz_error")
            }
        }
    }

    pub async fn batch_check(&self, request: &AuthzBatchCheckRequest) -> AuthzBatchCheckResponse {
        let mut decisions = Vec::with_capacity(request.checks.len());
        for check in &request.checks {
            decisions.push(self.check(check).await);
        }
        AuthzBatchCheckResponse { decisions }
    }

    async fn check_local(&self, request: &AuthzCheckRequest) -> Result<AuthzDecision, sqlx::Error> {
        let actor = self.actor_context(request).await?;
        let risk_level = request
            .context
            .as_ref()
            .and_then(|ctx| ctx.risk_level.as_deref())
            .unwrap_or("low");

        if self
            .has_explicit_binding(request, &actor, BindingEffect::Deny)
            .await?
        {
            return Ok(AuthzDecision::deny(
                LOCAL_POLICY_VERSION,
                "policy_explicit_deny",
            ));
        }

        if is_critical_without_explicit_policy(request, risk_level)
            && !self
                .has_explicit_binding(request, &actor, BindingEffect::Allow)
                .await?
        {
            return Ok(AuthzDecision::deny(
                LOCAL_POLICY_VERSION,
                "critical_risk_requires_explicit_policy",
            ));
        }

        if self
            .has_explicit_binding(request, &actor, BindingEffect::Review)
            .await?
        {
            return Ok(AuthzDecision::review(
                LOCAL_POLICY_VERSION,
                "policy_requires_review",
                Some("resource-policy-review".to_string()),
            ));
        }

        if should_review_by_risk(risk_level) {
            let may_execute = Self::has_admin_role(&actor)
                || self
                    .has_explicit_binding(request, &actor, BindingEffect::Allow)
                    .await?
                || self.has_matching_relation(request, &actor).await?;
            if may_execute {
                return Ok(AuthzDecision::review(
                    LOCAL_POLICY_VERSION,
                    "risk_requires_review",
                    Some("risk-review".to_string()),
                ));
            }
            return Ok(AuthzDecision::deny(
                LOCAL_POLICY_VERSION,
                "relation_missing",
            ));
        }

        if Self::has_admin_role(&actor) {
            return Ok(AuthzDecision::allow(LOCAL_POLICY_VERSION));
        }

        if self
            .has_explicit_binding(request, &actor, BindingEffect::Allow)
            .await?
        {
            return Ok(AuthzDecision::allow(LOCAL_POLICY_VERSION));
        }

        if self.has_matching_relation(request, &actor).await? {
            return Ok(AuthzDecision::allow(LOCAL_POLICY_VERSION));
        }

        if default_tenant_member_allow(request, &actor) {
            return Ok(AuthzDecision::allow(LOCAL_POLICY_VERSION));
        }

        Ok(AuthzDecision::deny(
            LOCAL_POLICY_VERSION,
            "relation_missing",
        ))
    }

    async fn actor_context(
        &self,
        request: &AuthzCheckRequest,
    ) -> Result<ActorAuthorizationContext, sqlx::Error> {
        let mut roles = request
            .actor
            .roles
            .iter()
            .map(|role| role.trim().to_string())
            .filter(|role| !role.is_empty())
            .collect::<HashSet<_>>();

        if let Some(session_id) = request.actor.session_id {
            let session_roles = sqlx::query(
                r#"
                SELECT roles_snapshot
                FROM platform_sessions
                WHERE id = $1
                  AND tenant_id = $2
                  AND user_id = $3
                  AND revoked_at IS NULL
                  AND (token_exp IS NULL OR token_exp > CURRENT_TIMESTAMP)
                "#,
            )
            .bind(session_id)
            .bind(request.tenant_id)
            .bind(request.actor.user_id)
            .fetch_optional(&self.pool)
            .await?;

            if let Some(row) = session_roles {
                let roles_snapshot: Value = row.try_get("roles_snapshot")?;
                extend_roles_from_json(&mut roles, &roles_snapshot);
            }
        }

        let membership = sqlx::query(
            r#"
            SELECT role
            FROM user_tenant_memberships
            WHERE tenant_id = $1 AND user_id = $2
            "#,
        )
        .bind(request.tenant_id)
        .bind(request.actor.user_id)
        .fetch_optional(&self.pool)
        .await?;

        let tenant_membership_role = match membership {
            Some(row) => {
                let role: String = row.try_get("role")?;
                match role.as_str() {
                    "admin" => {
                        roles.insert("tenant_admin".to_string());
                        roles.insert("tenant_member".to_string());
                    }
                    "member" => {
                        roles.insert("tenant_member".to_string());
                    }
                    other => {
                        roles.insert(other.to_string());
                    }
                }
                Some(role)
            }
            None => None,
        };

        Ok(ActorAuthorizationContext {
            roles,
            tenant_membership_role,
        })
    }

    fn has_admin_role(actor: &ActorAuthorizationContext) -> bool {
        actor.roles.iter().any(|role| {
            matches!(
                role.as_str(),
                "platform_admin"
                    | "tenant_admin"
                    | "security_admin"
                    | "audit_admin"
                    | "agent_admin"
                    | "skill_admin"
                    | "mcp_admin"
                    | "tool_admin"
                    | "workflow_admin"
                    | "memory_admin"
                    | "project_admin"
                    | "local_exec_admin"
            )
        }) || actor.tenant_membership_role.as_deref() == Some("admin")
    }

    async fn has_explicit_binding(
        &self,
        request: &AuthzCheckRequest,
        actor: &ActorAuthorizationContext,
        effect: BindingEffect,
    ) -> Result<bool, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT subject_type, subject_id, effect
            FROM resource_policy_bindings
            WHERE tenant_id = $1
              AND resource_type = $2
              AND resource_id = $3
              AND (action = $4 OR action = '*')
              AND disabled_at IS NULL
            ORDER BY created_at DESC
            "#,
        )
        .bind(request.tenant_id)
        .bind(&request.resource.resource_type)
        .bind(&request.resource.id)
        .bind(&request.action)
        .fetch_all(&self.pool)
        .await?;

        for row in rows {
            let row_effect = match row.try_get::<String, _>("effect")?.as_str() {
                "allow" => BindingEffect::Allow,
                "deny" => BindingEffect::Deny,
                "review" => BindingEffect::Review,
                _ => continue,
            };
            if row_effect != effect {
                continue;
            }

            let subject_type: String = row.try_get("subject_type")?;
            let subject_id: String = row.try_get("subject_id")?;
            if self
                .subject_matches(request, actor, subject_type.as_str(), subject_id.as_str())
                .await?
            {
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn subject_matches(
        &self,
        request: &AuthzCheckRequest,
        actor: &ActorAuthorizationContext,
        subject_type: &str,
        subject_id: &str,
    ) -> Result<bool, sqlx::Error> {
        match subject_type {
            "user" => Ok(subject_id == request.actor.user_id.to_string()),
            "role" => Ok(actor.roles.contains(subject_id)),
            "relation" => {
                let exists: bool = sqlx::query(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM resource_relations
                        WHERE tenant_id = $1
                          AND resource_type = $2
                          AND resource_id = $3
                          AND relation = $4
                          AND disabled_at IS NULL
                          AND (
                              (subject_type = 'user' AND subject_id = $5)
                              OR (subject_type = 'role' AND subject_id = ANY($6::text[]))
                          )
                    ) AS exists
                    "#,
                )
                .bind(request.tenant_id)
                .bind(&request.resource.resource_type)
                .bind(&request.resource.id)
                .bind(subject_id)
                .bind(request.actor.user_id.to_string())
                .bind(actor.roles.iter().cloned().collect::<Vec<_>>())
                .fetch_one(&self.pool)
                .await?
                .try_get("exists")?;
                Ok(exists)
            }
            _ => Ok(false),
        }
    }

    async fn has_matching_relation(
        &self,
        request: &AuthzCheckRequest,
        actor: &ActorAuthorizationContext,
    ) -> Result<bool, sqlx::Error> {
        let allowed_relations = relations_for_action(&request.action);
        if allowed_relations.is_empty() {
            return Ok(false);
        }

        let exists: bool = sqlx::query(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM resource_relations
                WHERE tenant_id = $1
                  AND resource_type = $2
                  AND resource_id = $3
                  AND relation = ANY($4::text[])
                  AND disabled_at IS NULL
                  AND (
                      (subject_type = 'user' AND subject_id = $5)
                      OR (subject_type = 'role' AND subject_id = ANY($6::text[]))
                  )
            ) AS exists
            "#,
        )
        .bind(request.tenant_id)
        .bind(&request.resource.resource_type)
        .bind(&request.resource.id)
        .bind(allowed_relations)
        .bind(request.actor.user_id.to_string())
        .bind(actor.roles.iter().cloned().collect::<Vec<_>>())
        .fetch_one(&self.pool)
        .await?
        .try_get("exists")?;

        Ok(exists)
    }
}

fn default_tenant_member_allow(
    request: &AuthzCheckRequest,
    actor: &ActorAuthorizationContext,
) -> bool {
    if !actor.roles.contains("tenant_member") {
        return false;
    }

    match (
        request.action.as_str(),
        request.resource.resource_type.as_str(),
    ) {
        ("create", "conversation")
        | ("read", "conversation")
        | ("subscribe", "conversation")
        | ("read", "run")
        | ("cancel", "run")
        | ("read", "project")
        | ("read", "file")
        | ("run", "conversation") => true,
        ("read" | "update", "memory") => request.resource.id == request.actor.user_id.to_string(),
        _ => false,
    }
}

fn extend_roles_from_json(roles: &mut HashSet<String>, value: &Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                if let Some(role) = item.as_str() {
                    roles.insert(role.to_string());
                }
            }
        }
        Value::Object(map) => {
            if let Some(Value::Array(items)) = map.get("roles") {
                for item in items {
                    if let Some(role) = item.as_str() {
                        roles.insert(role.to_string());
                    }
                }
            }
            if let Some(Value::Object(realm_access)) = map.get("realm_access")
                && let Some(Value::Array(items)) = realm_access.get("roles")
            {
                for item in items {
                    if let Some(role) = item.as_str() {
                        roles.insert(role.to_string());
                    }
                }
            }
        }
        _ => {}
    }
}

fn relations_for_action(action: &str) -> Vec<String> {
    let relations = match action {
        "manage" | "create" | "update" | "delete" | "publish" | "disable" => {
            vec!["owner", "admin", "manager"]
        }
        "approve" => vec!["approver", "admin", "owner"],
        "run" => vec!["runner", "operator", "user", "member", "owner", "admin"],
        "execute" => vec!["user", "runner", "operator", "member", "owner", "admin"],
        "read" | "use" | "subscribe" => {
            vec!["viewer", "user", "member", "owner", "admin", "runner"]
        }
        "write" => vec!["writer", "editor", "member", "owner", "admin"],
        "cancel" => vec!["owner", "admin", "operator"],
        _ => vec![],
    };
    relations.into_iter().map(str::to_string).collect()
}

fn should_review_by_risk(risk_level: &str) -> bool {
    matches!(risk_level, "high" | "critical")
}

fn is_critical_without_explicit_policy(request: &AuthzCheckRequest, risk_level: &str) -> bool {
    risk_level == "critical"
        && matches!(
            (
                request.action.as_str(),
                request.resource.resource_type.as_str()
            ),
            ("execute", "local_exec") | ("execute", "sql_tool") | ("execute", "sql_query")
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::agent_platform::models::{ActorRef, ResourceRef};
    use uuid::Uuid;

    fn tenant_member() -> ActorAuthorizationContext {
        ActorAuthorizationContext {
            roles: HashSet::from(["tenant_member".to_string()]),
            tenant_membership_role: Some("member".to_string()),
        }
    }

    fn request(action: &str, resource_type: &str, resource_id: String) -> AuthzCheckRequest {
        AuthzCheckRequest {
            tenant_id: Uuid::new_v4(),
            actor: ActorRef {
                user_id: Uuid::new_v4(),
                device_id: None,
                session_id: None,
                roles: Vec::new(),
            },
            action: action.to_string(),
            resource: ResourceRef {
                resource_type: resource_type.to_string(),
                id: resource_id,
                path: None,
            },
            context: None,
        }
    }

    #[test]
    fn tenant_member_default_allows_only_own_memory() {
        let mut req = request("read", "memory", String::new());
        req.resource.id = req.actor.user_id.to_string();

        assert!(default_tenant_member_allow(&req, &tenant_member()));

        req.resource.id = Uuid::new_v4().to_string();

        assert!(!default_tenant_member_allow(&req, &tenant_member()));
    }

    #[test]
    fn tenant_member_default_does_not_allow_file_write() {
        let req = request("write", "file", "project:path".to_string());

        assert!(!default_tenant_member_allow(&req, &tenant_member()));
    }

    #[test]
    fn audit_admin_is_treated_as_admin_role() {
        let actor = ActorAuthorizationContext {
            roles: HashSet::from(["audit_admin".to_string()]),
            tenant_membership_role: Some("member".to_string()),
        };

        assert!(ResourceAuthzService::has_admin_role(&actor));
    }
}
