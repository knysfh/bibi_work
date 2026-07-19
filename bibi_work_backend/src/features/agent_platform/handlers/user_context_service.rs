use std::collections::BTreeSet;

use axum::{Extension, Json, extract::State};
use sqlx::Row;

use crate::{
    features::{
        agent_platform::{ferriskey_oidc::PlatformRequestContext, models::*},
        core::errors::AppError,
    },
    startup::AppState,
};
use uuid::Uuid;

pub async fn get_me(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<MeResponse>, AppError> {
    Ok(Json(load_current_me(&state, &ctx).await?))
}

pub(super) async fn load_current_me(
    state: &AppState,
    ctx: &PlatformRequestContext,
) -> Result<MeResponse, AppError> {
    let user_row = sqlx::query(
        r#"
        SELECT id, tenant_id, ferriskey_subject, username, email, display_name,
               status, created_at, updated_at
        FROM platform_users
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(ctx.platform_user_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("current user projection not found".to_string()))?;

    let tenant_rows = sqlx::query(
        r#"
        SELECT t.id, t.name, t.slug, t.metadata, m.role AS membership_role
        FROM user_tenant_memberships m
        JOIN tenants t ON t.id = m.tenant_id
        WHERE m.user_id = $1
          AND t.deleted_at IS NULL
        ORDER BY t.name ASC, t.slug ASC
        "#,
    )
    .bind(ctx.platform_user_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let mut current_membership_role = None;
    for row in &tenant_rows {
        let tenant_id: Uuid = row.try_get("id")?;
        if tenant_id == ctx.tenant_id {
            current_membership_role = Some(row.try_get::<String, _>("membership_role")?);
            break;
        }
    }
    let effective_roles =
        effective_roles_for_membership(&ctx.roles, current_membership_role.as_deref());
    let capabilities = capabilities_for_roles(&effective_roles);

    let device_row = sqlx::query(
        r#"
        SELECT id, tenant_id, device_name, platform, trust_level, last_seen_at, revoked_at
        FROM devices
        WHERE id = $1 AND tenant_id = $2 AND user_id = $3
        "#,
    )
    .bind(ctx.device_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("current device projection not found".to_string()))?;

    let session_row = sqlx::query(
        r#"
        SELECT id, tenant_id, device_id, token_exp, last_seen_at,
               source_ip, user_agent, revoked_at
        FROM platform_sessions
        WHERE id = $1 AND tenant_id = $2 AND user_id = $3
        "#,
    )
    .bind(ctx.session_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("current session projection not found".to_string()))?;

    let tenants = tenant_rows
        .into_iter()
        .map(|row| {
            Ok(MeTenantResponse {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                slug: row.try_get("slug")?,
                membership_role: row.try_get("membership_role")?,
                metadata: row.try_get("metadata")?,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;

    Ok(MeResponse {
        tenant_id: ctx.tenant_id,
        user: MeUserResponse {
            id: user_row.try_get("id")?,
            tenant_id: user_row.try_get("tenant_id")?,
            ferriskey_subject: user_row.try_get("ferriskey_subject")?,
            username: user_row.try_get("username")?,
            email: user_row.try_get("email")?,
            display_name: user_row.try_get("display_name")?,
            status: user_row.try_get("status")?,
            created_at: user_row.try_get("created_at")?,
            updated_at: user_row.try_get("updated_at")?,
        },
        tenants,
        capabilities,
        roles: effective_roles,
        device: MeDeviceResponse {
            id: device_row.try_get("id")?,
            tenant_id: device_row.try_get("tenant_id")?,
            device_name: device_row.try_get("device_name")?,
            platform: device_row.try_get("platform")?,
            trust_level: device_row.try_get("trust_level")?,
            last_seen_at: device_row.try_get("last_seen_at")?,
            revoked_at: device_row.try_get("revoked_at")?,
        },
        session: MeSessionResponse {
            id: session_row.try_get("id")?,
            tenant_id: session_row.try_get("tenant_id")?,
            device_id: session_row.try_get("device_id")?,
            token_exp: session_row.try_get("token_exp")?,
            last_seen_at: session_row.try_get("last_seen_at")?,
            source_ip: session_row.try_get("source_ip")?,
            user_agent: session_row.try_get("user_agent")?,
            revoked_at: session_row.try_get("revoked_at")?,
        },
    })
}

fn effective_roles_for_membership(roles: &[String], membership_role: Option<&str>) -> Vec<String> {
    let mut roles = roles
        .iter()
        .map(|role| role.trim())
        .filter(|role| !role.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>();

    if let Some(role) = membership_role.map(str::trim) {
        match role {
            "admin" => {
                roles.insert("tenant_admin".to_string());
                roles.insert("tenant_member".to_string());
            }
            "member" => {
                roles.insert("tenant_member".to_string());
            }
            "" => {}
            other => {
                roles.insert(other.to_string());
            }
        }
    }

    roles.into_iter().collect()
}

fn capabilities_for_roles(roles: &[String]) -> Vec<String> {
    let mut capabilities = BTreeSet::from([
        "conversation:create".to_string(),
        "conversation:read".to_string(),
        "project:read".to_string(),
        "run:read".to_string(),
    ]);

    for role in roles {
        match role.as_str() {
            "platform_admin" | "tenant_admin" => {
                capabilities.insert("tenant:manage".to_string());
                capabilities.insert("catalog:manage".to_string());
                capabilities.insert("workflow:manage".to_string());
                capabilities.insert("memory:govern".to_string());
                capabilities.insert("approval:decide".to_string());
                capabilities.insert("audit:read".to_string());
            }
            "agent_admin" | "skill_admin" | "tool_admin" | "mcp_admin" => {
                capabilities.insert("catalog:manage".to_string());
            }
            "workflow_admin" => {
                capabilities.insert("workflow:manage".to_string());
            }
            "workflow_operator" => {
                capabilities.insert("workflow:run".to_string());
            }
            "agent_runner" => {
                capabilities.insert("agent:run".to_string());
            }
            "memory_admin" => {
                capabilities.insert("memory:govern".to_string());
            }
            "audit_admin" => {
                capabilities.insert("audit:read".to_string());
            }
            "security_admin" => {
                capabilities.insert("approval:decide".to_string());
                capabilities.insert("audit:read".to_string());
            }
            _ => {}
        }
    }

    capabilities.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_membership_exposes_admin_navigation_capabilities() {
        let roles = effective_roles_for_membership(&[], Some("admin"));
        assert!(roles.contains(&"tenant_admin".to_string()));
        assert!(roles.contains(&"tenant_member".to_string()));

        let capabilities = capabilities_for_roles(&roles);
        assert!(capabilities.contains(&"catalog:manage".to_string()));
        assert!(capabilities.contains(&"workflow:manage".to_string()));
        assert!(capabilities.contains(&"memory:govern".to_string()));
        assert!(capabilities.contains(&"audit:read".to_string()));
    }

    #[test]
    fn member_membership_does_not_expose_admin_capabilities() {
        let roles = effective_roles_for_membership(&[], Some("member"));
        let capabilities = capabilities_for_roles(&roles);

        assert!(roles.contains(&"tenant_member".to_string()));
        assert!(!capabilities.contains(&"catalog:manage".to_string()));
        assert!(!capabilities.contains(&"audit:read".to_string()));
    }

    #[test]
    fn token_roles_are_trimmed_and_deduplicated() {
        let roles = effective_roles_for_membership(
            &[
                " agent_admin ".to_string(),
                "agent_admin".to_string(),
                "".to_string(),
            ],
            None,
        );

        assert_eq!(roles, vec!["agent_admin".to_string()]);
    }
}
