use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{ferriskey_oidc::PlatformRequestContext, models::AuthzContext},
        core::errors::AppError,
    },
    startup::AppState,
};

use super::support::require_ferriskey_allow;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapabilityAuthzRequirement {
    resource_type: &'static str,
    resource_id: Uuid,
    action: &'static str,
    tool_id: Option<Uuid>,
    mcp_server_id: Option<Uuid>,
}

pub(super) async fn require_agent_version_capabilities(
    state: &AppState,
    ctx: &PlatformRequestContext,
    tenant_id: Uuid,
    agent_version_id: Uuid,
    base_context: AuthzContext,
) -> Result<(), AppError> {
    let requirements = load_agent_version_capability_requirements(
        &state.connect_pool,
        tenant_id,
        agent_version_id,
    )
    .await?;

    for requirement in requirements {
        let mut context = base_context.clone();
        context.tool_id = requirement.tool_id;
        context.mcp_server_id = requirement.mcp_server_id;
        require_ferriskey_allow(
            state,
            ctx,
            tenant_id,
            requirement.action,
            requirement.resource_type,
            requirement.resource_id.to_string(),
            Some(context),
        )
        .await?;
    }

    Ok(())
}

async fn load_agent_version_capability_requirements(
    pool: &PgPool,
    tenant_id: Uuid,
    agent_version_id: Uuid,
) -> Result<Vec<CapabilityAuthzRequirement>, AppError> {
    let mut requirements = Vec::new();

    let skill_rows = sqlx::query(
        r#"
        SELECT s.id AS skill_id
        FROM agent_version_skill_bindings b
        JOIN skill_versions sv ON sv.id = b.skill_version_id
        JOIN skills s ON s.id = sv.skill_id
        WHERE b.agent_version_id = $1
          AND sv.tenant_id = $2
          AND sv.status = 'published'
          AND s.status = 'active'
          AND s.deleted_at IS NULL
        ORDER BY b.created_at ASC, s.id ASC
        "#,
    )
    .bind(agent_version_id)
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    for row in skill_rows {
        requirements.push(CapabilityAuthzRequirement {
            resource_type: "skill",
            resource_id: row.try_get("skill_id")?,
            action: "use",
            tool_id: None,
            mcp_server_id: None,
        });
    }

    let tool_rows = sqlx::query(
        r#"
        SELECT t.id AS tool_id
        FROM agent_version_tool_bindings b
        JOIN tool_versions tv ON tv.id = b.tool_version_id
        JOIN tools t ON t.id = tv.tool_id
        WHERE b.agent_version_id = $1
          AND tv.tenant_id = $2
          AND tv.status = 'published'
          AND t.status = 'active'
          AND t.deleted_at IS NULL
        ORDER BY b.created_at ASC, t.id ASC
        "#,
    )
    .bind(agent_version_id)
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    for row in tool_rows {
        let tool_id = row.try_get("tool_id")?;
        requirements.push(CapabilityAuthzRequirement {
            resource_type: "tool",
            resource_id: tool_id,
            action: "use",
            tool_id: Some(tool_id),
            mcp_server_id: None,
        });
    }

    let sql_tool_rows = sqlx::query(
        r#"
        SELECT stv.sql_tool_id
        FROM agent_version_sql_tool_bindings b
        JOIN sql_tool_versions stv ON stv.id = b.sql_tool_version_id
        JOIN sql_tools st ON st.id = stv.sql_tool_id
        JOIN sql_connections sc ON sc.id = stv.connection_id
        WHERE b.agent_version_id = $1
          AND stv.tenant_id = $2
          AND stv.status = 'published'
          AND st.tenant_id = $2
          AND st.status = 'active'
          AND sc.tenant_id = $2
          AND sc.status = 'active'
        ORDER BY b.created_at ASC, stv.sql_tool_id ASC
        "#,
    )
    .bind(agent_version_id)
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    for row in sql_tool_rows {
        requirements.push(CapabilityAuthzRequirement {
            resource_type: "sql_tool",
            resource_id: row.try_get("sql_tool_id")?,
            action: "use",
            tool_id: None,
            mcp_server_id: None,
        });
    }

    let mcp_rows = sqlx::query(
        r#"
        SELECT mt.id AS mcp_tool_id, mt.mcp_server_id
        FROM agent_version_mcp_bindings b
        JOIN mcp_tools mt ON mt.id = b.mcp_tool_id
        JOIN mcp_servers ms ON ms.id = mt.mcp_server_id
        WHERE b.agent_version_id = $1
          AND mt.tenant_id = $2
          AND mt.status = 'active'
          AND ms.status = 'active'
          AND ms.deleted_at IS NULL
        ORDER BY b.created_at ASC, mt.id ASC
        "#,
    )
    .bind(agent_version_id)
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    for row in mcp_rows {
        requirements.push(CapabilityAuthzRequirement {
            resource_type: "mcp_tool",
            resource_id: row.try_get("mcp_tool_id")?,
            action: "use",
            tool_id: None,
            mcp_server_id: Some(row.try_get("mcp_server_id")?),
        });
    }

    Ok(deduplicate_requirements(requirements))
}

fn deduplicate_requirements(
    requirements: Vec<CapabilityAuthzRequirement>,
) -> Vec<CapabilityAuthzRequirement> {
    let mut unique: Vec<CapabilityAuthzRequirement> = Vec::new();
    for requirement in requirements {
        if !unique.iter().any(|existing| {
            existing.resource_type == requirement.resource_type
                && existing.resource_id == requirement.resource_id
                && existing.action == requirement.action
        }) {
            unique.push(requirement);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    #[test]
    fn deduplicate_requirements_keeps_first_unique_resource_action() {
        let skill_id = Uuid::new_v4();
        let tool_id = Uuid::new_v4();
        let requirements = deduplicate_requirements(vec![
            CapabilityAuthzRequirement {
                resource_type: "skill",
                resource_id: skill_id,
                action: "use",
                tool_id: None,
                mcp_server_id: None,
            },
            CapabilityAuthzRequirement {
                resource_type: "skill",
                resource_id: skill_id,
                action: "use",
                tool_id: None,
                mcp_server_id: None,
            },
            CapabilityAuthzRequirement {
                resource_type: "tool",
                resource_id: tool_id,
                action: "use",
                tool_id: Some(tool_id),
                mcp_server_id: None,
            },
        ]);

        assert_eq!(requirements.len(), 2);
        assert_eq!(requirements[0].resource_type, "skill");
        assert_eq!(requirements[1].tool_id, Some(tool_id));
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn loads_agent_version_capability_requirements_from_postgres()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = test_pool().await?;
        let tenant_id = Uuid::new_v4();
        let agent_id = Uuid::new_v4();
        let agent_version_id = Uuid::new_v4();
        let skill_id = Uuid::new_v4();
        let first_skill_version_id = Uuid::new_v4();
        let second_skill_version_id = Uuid::new_v4();
        let tool_id = Uuid::new_v4();
        let tool_version_id = Uuid::new_v4();
        let sql_connection_id = Uuid::new_v4();
        let sql_tool_id = Uuid::new_v4();
        let sql_tool_version_id = Uuid::new_v4();
        let mcp_server_id = Uuid::new_v4();
        let mcp_tool_id = Uuid::new_v4();

        sqlx::query(
            r#"
            INSERT INTO tenants (id, name, slug)
            VALUES ($1, 'capability authz test tenant', $2)
            "#,
        )
        .bind(tenant_id)
        .bind(format!("capability-authz-{tenant_id}"))
        .execute(&pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO agents (id, tenant_id, name, status)
            VALUES ($1, $2, 'capability-agent', 'active')
            "#,
        )
        .bind(agent_id)
        .bind(tenant_id)
        .execute(&pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO agent_versions (id, tenant_id, agent_id, version_label, status)
            VALUES ($1, $2, $3, 'v1', 'published')
            "#,
        )
        .bind(agent_version_id)
        .bind(tenant_id)
        .bind(agent_id)
        .execute(&pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO skills (id, tenant_id, name, status)
            VALUES ($1, $2, 'capability-skill', 'active')
            "#,
        )
        .bind(skill_id)
        .bind(tenant_id)
        .execute(&pool)
        .await?;

        for (skill_version_id, version_label, status) in [
            (first_skill_version_id, "v1", "published"),
            (second_skill_version_id, "v2", "disabled"),
        ] {
            sqlx::query(
                r#"
                INSERT INTO skill_versions (id, tenant_id, skill_id, version_label, status)
                VALUES ($1, $2, $3, $4, $5)
                "#,
            )
            .bind(skill_version_id)
            .bind(tenant_id)
            .bind(skill_id)
            .bind(version_label)
            .bind(status)
            .execute(&pool)
            .await?;
            sqlx::query(
                r#"
                INSERT INTO agent_version_skill_bindings (agent_version_id, skill_version_id)
                VALUES ($1, $2)
                "#,
            )
            .bind(agent_version_id)
            .bind(skill_version_id)
            .execute(&pool)
            .await?;
        }

        sqlx::query(
            r#"
            INSERT INTO tools (id, tenant_id, name, status)
            VALUES ($1, $2, 'capability-tool', 'active')
            "#,
        )
        .bind(tool_id)
        .bind(tenant_id)
        .execute(&pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO tool_versions (id, tenant_id, tool_id, version_label, status)
            VALUES ($1, $2, $3, 'v1', 'published')
            "#,
        )
        .bind(tool_version_id)
        .bind(tenant_id)
        .bind(tool_id)
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO agent_version_tool_bindings (agent_version_id, tool_version_id)
            VALUES ($1, $2)
            "#,
        )
        .bind(agent_version_id)
        .bind(tool_version_id)
        .execute(&pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO sql_connections (
                id, tenant_id, name, database_kind, host, port, database_name, status
            )
            VALUES ($1, $2, 'capability-sql-conn', 'postgres', '127.0.0.1', 5433, 'bibi_work', 'active')
            "#,
        )
        .bind(sql_connection_id)
        .bind(tenant_id)
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO sql_tools (id, tenant_id, name, status)
            VALUES ($1, $2, 'capability-sql-tool', 'active')
            "#,
        )
        .bind(sql_tool_id)
        .bind(tenant_id)
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO sql_tool_versions (
                id, tenant_id, sql_tool_id, connection_id, version_label,
                operation, sql_template, query_hash, status
            )
            VALUES (
                $1, $2, $3, $4, 'v1', 'read', 'SELECT 1', 'sha256:capability-sql', 'published'
            )
            "#,
        )
        .bind(sql_tool_version_id)
        .bind(tenant_id)
        .bind(sql_tool_id)
        .bind(sql_connection_id)
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO agent_version_sql_tool_bindings (
                agent_version_id, sql_tool_version_id
            )
            VALUES ($1, $2)
            "#,
        )
        .bind(agent_version_id)
        .bind(sql_tool_version_id)
        .execute(&pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO mcp_servers (id, tenant_id, name, status)
            VALUES ($1, $2, 'capability-mcp', 'active')
            "#,
        )
        .bind(mcp_server_id)
        .bind(tenant_id)
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO mcp_tools (id, tenant_id, mcp_server_id, name, status)
            VALUES ($1, $2, $3, 'capability-mcp-tool', 'active')
            "#,
        )
        .bind(mcp_tool_id)
        .bind(tenant_id)
        .bind(mcp_server_id)
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO agent_version_mcp_bindings (agent_version_id, mcp_tool_id)
            VALUES ($1, $2)
            "#,
        )
        .bind(agent_version_id)
        .bind(mcp_tool_id)
        .execute(&pool)
        .await?;

        let requirements =
            load_agent_version_capability_requirements(&pool, tenant_id, agent_version_id).await?;

        assert_eq!(requirements.len(), 4);
        assert!(requirements.iter().any(|requirement| {
            requirement.resource_type == "skill"
                && requirement.resource_id == skill_id
                && requirement.action == "use"
                && requirement.tool_id.is_none()
                && requirement.mcp_server_id.is_none()
        }));
        assert!(requirements.iter().any(|requirement| {
            requirement.resource_type == "tool"
                && requirement.resource_id == tool_id
                && requirement.action == "use"
                && requirement.tool_id == Some(tool_id)
        }));
        assert!(requirements.iter().any(|requirement| {
            requirement.resource_type == "mcp_tool"
                && requirement.resource_id == mcp_tool_id
                && requirement.action == "use"
                && requirement.mcp_server_id == Some(mcp_server_id)
        }));
        assert!(requirements.iter().any(|requirement| {
            requirement.resource_type == "sql_tool"
                && requirement.resource_id == sql_tool_id
                && requirement.action == "use"
                && requirement.tool_id.is_none()
                && requirement.mcp_server_id.is_none()
        }));

        cleanup_tenant(&pool, tenant_id).await?;
        Ok(())
    }

    async fn test_pool() -> Result<PgPool, Box<dyn std::error::Error>> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://postgres:password@127.0.0.1:5433/bibi_work".to_string()
        });
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(pool)
    }

    async fn cleanup_tenant(
        pool: &PgPool,
        tenant_id: Uuid,
    ) -> Result<(), Box<dyn std::error::Error>> {
        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(pool)
            .await?;
        Ok(())
    }
}
