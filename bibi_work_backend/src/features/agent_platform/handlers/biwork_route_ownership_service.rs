use axum::Json;
use serde_json::{Value, json};

use crate::features::core::errors::AppError;

use super::biwork_compat_service::ok;

pub(super) fn route_ownership_manifest() -> Value {
    let prefixes = json!({
        "auth": "RUST",
        "me": "RUST",
        "settings": "RUST",
        "system": "RUST",
        "webui": "RUST",
        "google": "RUST",
        "bedrock": "RUST",
        "routeOwnership": "RUST",
        "assistants": "RUST",
        "agents": "RUST",
        "agents.management": "RUST",
        "agents.custom": "AGGREGATE",
        "skills": "RUST",
        "mcp": "RUST",
        "providers": "RUST",
        "remoteAgents": "RUST",
        "workbench": "RUST",
        "conversations": "RUST",
        "messages": "RUST",
        "teams": "RUST",
        "workflow": "RUST",
        "fs": "FACADE",
        "cron": "RUST",
        "channel.plugins": "AGGREGATE",
        "channel.facts": "RUST",
        "extensions": "AGGREGATE",
        "hub": "AGGREGATE",
        "shell": "LOCAL",
        "officePreview": "LOCAL",
        "stt": "RUST",
        "ws": "AGGREGATE",
    });
    let mut routes = json!([
        { "method": "GET", "path": "/api/auth/status", "ownership": "RUST", "authority": "rust-compat", "auth": "public" },
        { "method": "GET", "path": "/api/auth/oidc/config", "ownership": "RUST", "authority": "rust-compat", "auth": "public" },
        { "method": "POST", "path": "/api/auth/oidc/token", "ownership": "RUST", "authority": "rust-compat", "auth": "public" },
        { "method": "GET", "path": "/api/auth/user", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/auth/logout", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/me", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/settings/client", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "PUT", "path": "/api/settings/client", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "PATCH", "path": "/api/settings", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/system/info", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/assistants", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/agents/management", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/agents/custom", "ownership": "AGGREGATE", "authority": "desktop-gateway+rust-governance", "auth": "bearer" },
        { "method": "GET", "path": "/api/skills", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/skills", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/skills/builtin-rule", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/skills/builtin-skill", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/mcp/servers", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/providers", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/workbench/bootstrap", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/conversations", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/conversations", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/conversations/{conversation_id}/messages", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/conversations/{conversation_id}/messages", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/conversations/{conversation_id}/cancel", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/conversations/{conversation_id}/runtime/ensure", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/conversations/{conversation_id}/workspace", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/conversations/{conversation_id}/confirmations", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/conversations/{conversation_id}/confirmations/{call_id}/confirm", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/teams", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/teams", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/teams/{team_id}/messages", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/dir", "ownership": "FACADE", "authority": "desktop-gateway-local|rust-file-service", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/list", "ownership": "FACADE", "authority": "desktop-gateway-local|rust-file-service", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/read", "ownership": "FACADE", "authority": "desktop-gateway-local|rust-file-service", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/read-buffer", "ownership": "FACADE", "authority": "desktop-gateway-local|rust-file-service", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/image-base64", "ownership": "FACADE", "authority": "desktop-gateway-local|rust-file-service", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/write", "ownership": "FACADE", "authority": "desktop-gateway-local|rust-file-service", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/metadata", "ownership": "FACADE", "authority": "desktop-gateway-local|rust-file-service", "auth": "bearer" },
        { "method": "GET", "path": "/api/fs/browse", "ownership": "FACADE", "authority": "desktop-gateway-local-picker", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/upload", "ownership": "FACADE", "authority": "desktop-gateway-local|rust-file-service", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/temp", "ownership": "FACADE", "authority": "desktop-gateway-local", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/zip", "ownership": "LOCAL", "authority": "desktop-gateway-local-zip", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/zip/cancel", "ownership": "LOCAL", "authority": "desktop-gateway-local-zip", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/copy", "ownership": "FACADE", "authority": "desktop-gateway-local", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/remove", "ownership": "FACADE", "authority": "desktop-gateway-local", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/rename", "ownership": "FACADE", "authority": "desktop-gateway-local", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/watch/start", "ownership": "LOCAL", "authority": "desktop-gateway-local-watch", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/watch/stop", "ownership": "LOCAL", "authority": "desktop-gateway-local-watch", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/watch/stop-all", "ownership": "LOCAL", "authority": "desktop-gateway-local-watch", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/office-watch/start", "ownership": "LOCAL", "authority": "desktop-gateway-local-watch", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/office-watch/stop", "ownership": "LOCAL", "authority": "desktop-gateway-local-watch", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/snapshot/init", "ownership": "LOCAL", "authority": "desktop-gateway-local-snapshot", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/snapshot/compare", "ownership": "LOCAL", "authority": "desktop-gateway-local-snapshot", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/snapshot/baseline", "ownership": "LOCAL", "authority": "desktop-gateway-local-snapshot", "auth": "bearer" },
        { "method": "POST", "path": "/api/fs/snapshot/reset", "ownership": "LOCAL", "authority": "desktop-gateway-local-snapshot", "auth": "bearer" },
        { "method": "POST", "path": "/api/preview-history/list", "ownership": "LOCAL", "authority": "desktop-gateway-local-preview-history", "auth": "bearer" },
        { "method": "POST", "path": "/api/preview-history/save", "ownership": "LOCAL", "authority": "desktop-gateway-local-preview-history", "auth": "bearer" },
        { "method": "POST", "path": "/api/preview-history/get-content", "ownership": "LOCAL", "authority": "desktop-gateway-local-preview-history", "auth": "bearer" },
        { "method": "GET", "path": "/api/cron/jobs", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/cron/jobs", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/cron/jobs/{job_id}/run", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/cron/internal/system-resume", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/channel/plugins", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-channel-plugins+rust-governance", "auth": "bearer" },
        { "method": "POST", "path": "/api/channel/plugins/test", "ownership": "LOCAL", "authority": "desktop-gateway-local-channel-plugins", "auth": "bearer" },
        { "method": "POST", "path": "/api/channel/plugins/enable", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/channel/plugins/disable", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/channel/ingress/messages", "ownership": "RUST", "authority": "desktop-gateway-channel-connector", "auth": "bearer" },
        { "method": "GET", "path": "/api/channel/pairings", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/channel/pairings/request", "ownership": "RUST", "authority": "rust-compat|desktop-gateway-channel-connector", "auth": "bearer" },
        { "method": "POST", "path": "/api/channel/pairings/approve", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/channel/pairings/reject", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/channel/users", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/channel/users/revoke", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/channel/sessions", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/channel/settings/{platform}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "PUT", "path": "/api/channel/settings/{platform}/assistant", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "PUT", "path": "/api/channel/settings/{platform}/default-model", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "POST", "path": "/api/channel/settings/sync", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" },
        { "method": "GET", "path": "/api/extensions", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "GET", "path": "/api/extensions/themes", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "GET", "path": "/api/extensions/assistants", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "GET", "path": "/api/extensions/agents", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "GET", "path": "/api/extensions/acp-adapters", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "GET", "path": "/api/extensions/channel-plugins", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "GET", "path": "/api/extensions/mcp-servers", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "GET", "path": "/api/extensions/skills", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "GET", "path": "/api/extensions/settings-tabs", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "GET", "path": "/api/extensions/webui", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "GET", "path": "/api/extensions/agent-activity", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "POST", "path": "/api/extensions/sync", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "POST", "path": "/api/extensions/i18n", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "POST", "path": "/api/extensions/enable", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "POST", "path": "/api/extensions/disable", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "POST", "path": "/api/extensions/permissions", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "POST", "path": "/api/extensions/risk-level", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-extensions+rust-governance", "auth": "bearer" },
        { "method": "GET", "path": "/api/extensions/static/{extension_name}/{*asset_path}", "ownership": "LOCAL", "authority": "desktop-gateway-local-extension-static+rust-governance", "auth": "bearer" },
        { "method": "GET", "path": "/api/hub/extensions", "ownership": "AGGREGATE", "authority": "desktop-gateway-local-hub-index+rust-governance", "auth": "bearer" },
        { "method": "POST", "path": "/api/hub/install", "ownership": "LOCAL", "authority": "desktop-gateway-local-hub", "auth": "bearer" },
        { "method": "POST", "path": "/api/hub/uninstall", "ownership": "LOCAL", "authority": "desktop-gateway-local-hub", "auth": "bearer" },
        { "method": "POST", "path": "/api/hub/retry-install", "ownership": "LOCAL", "authority": "desktop-gateway-local-hub", "auth": "bearer" },
        { "method": "POST", "path": "/api/hub/check-updates", "ownership": "LOCAL", "authority": "desktop-gateway-local-hub", "auth": "bearer" },
        { "method": "POST", "path": "/api/hub/update", "ownership": "LOCAL", "authority": "desktop-gateway-local-hub", "auth": "bearer" },
        { "method": "POST", "path": "/api/shell/open-file", "ownership": "LOCAL", "authority": "desktop-local", "auth": "desktop-session" },
        { "method": "POST", "path": "/api/document/convert", "ownership": "LOCAL", "authority": "desktop-local", "auth": "desktop-session" },
        { "method": "POST", "path": "/api/ppt-preview/start", "ownership": "LOCAL", "authority": "desktop-local", "auth": "desktop-session" },
        { "method": "POST", "path": "/api/ppt-preview/stop", "ownership": "LOCAL", "authority": "desktop-local", "auth": "desktop-session" },
        { "method": "POST", "path": "/api/word-preview/start", "ownership": "LOCAL", "authority": "desktop-local", "auth": "desktop-session" },
        { "method": "POST", "path": "/api/word-preview/stop", "ownership": "LOCAL", "authority": "desktop-local", "auth": "desktop-session" },
        { "method": "POST", "path": "/api/excel-preview/start", "ownership": "LOCAL", "authority": "desktop-local", "auth": "desktop-session" },
        { "method": "POST", "path": "/api/excel-preview/stop", "ownership": "LOCAL", "authority": "desktop-local", "auth": "desktop-session" },
        { "method": "GET", "path": "/api/ppt-proxy/{port}", "ownership": "LOCAL", "authority": "desktop-local-office-proxy", "auth": "desktop-session" },
        { "method": "GET", "path": "/api/ppt-proxy/{port}/{*asset_path}", "ownership": "LOCAL", "authority": "desktop-local-office-proxy", "auth": "desktop-session" },
        { "method": "GET", "path": "/api/office-watch-proxy/{port}", "ownership": "LOCAL", "authority": "desktop-local-office-proxy", "auth": "desktop-session" },
        { "method": "GET", "path": "/api/office-watch-proxy/{port}/{*asset_path}", "ownership": "LOCAL", "authority": "desktop-local-office-proxy", "auth": "desktop-session" },
        { "method": "POST", "path": "/api/stt", "ownership": "RUST", "authority": "rust-compat-visible-degrade", "auth": "bearer" },
        { "method": "GET", "path": "/api/stt/stream", "ownership": "RUST", "authority": "rust-compat-visible-degrade", "auth": "bearer" },
        { "method": "WS", "path": "/ws", "ownership": "AGGREGATE", "authority": "desktop-ws-multiplexer|rust-enterprise-ws", "auth": "ws-auth-frame" }
    ]);
    routes
        .as_array_mut()
        .expect("route ownership routes must be an array")
        .extend([
            json!({ "method": "POST", "path": "/api/auth/oidc/revoke", "ownership": "RUST", "authority": "rust-compat", "auth": "public" }),
            json!({ "method": "POST", "path": "/api/system/ensure-node-runtime", "ownership": "RUST", "authority": "rust-compat-visible-degrade", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/system/ensure-managed-acp-tool", "ownership": "RUST", "authority": "rust-compat-visible-degrade", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/google/subscription-status", "ownership": "RUST", "authority": "rust-compat-visible-degrade", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/bedrock/test-connection", "ownership": "RUST", "authority": "rust-compat-visible-degrade", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/webui/change-password", "ownership": "RUST", "authority": "rust-compat-oidc", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/webui/change-username", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/webui/reset-password", "ownership": "RUST", "authority": "rust-compat-oidc", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/webui/generate-qr-token", "ownership": "RUST", "authority": "rust-compat-oidc", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/assistants", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/assistants/import", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/assistants/{assistant_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "PUT", "path": "/api/assistants/{assistant_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "DELETE", "path": "/api/assistants/{assistant_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "PATCH", "path": "/api/assistants/{assistant_id}/state", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/agents/refresh", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/agents/provider-health-check", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "PATCH", "path": "/api/agents/{agent_id}/enabled", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/agents/{agent_id}/health-check", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/agents/{agent_id}/overrides", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "PUT", "path": "/api/agents/{agent_id}/overrides", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/agents/{agent_id}/mcp-capabilities", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "PUT", "path": "/api/agents/{agent_id}/mcp-capabilities", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/agents/custom/try-connect", "ownership": "AGGREGATE", "authority": "desktop-gateway+rust-governance", "auth": "bearer" }),
            json!({ "method": "PUT", "path": "/api/agents/custom/{agent_id}", "ownership": "AGGREGATE", "authority": "desktop-gateway+rust-governance", "auth": "bearer" }),
            json!({ "method": "DELETE", "path": "/api/agents/custom/{agent_id}", "ownership": "AGGREGATE", "authority": "desktop-gateway+rust-governance", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/skills/materialize-for-agent", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/skills/assistant-rule/read", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/skills/assistant-rule/write", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "DELETE", "path": "/api/skills/assistant-rule/{assistant_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/skills/info", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/skills/import", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/skills/import-upload", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/skills/scan", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/skills/detect-paths", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/skills/detect-external", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/skills/import-history", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/skills/import-limits", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/skills/paths", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/skills/external-paths", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/skills/external-paths", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "DELETE", "path": "/api/skills/external-paths", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/skills/market/enable", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/skills/market/disable", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "DELETE", "path": "/api/skills/{skill_name}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/mcp/servers", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/mcp/servers/import", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "PUT", "path": "/api/mcp/servers/{server_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "DELETE", "path": "/api/mcp/servers/{server_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/mcp/servers/{server_id}/toggle", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/mcp/agent-configs", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/mcp/test-connection", "ownership": "FACADE", "authority": "desktop-stdio-mcp|rust-mcp-catalog", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/mcp/servers/{server_id}/local-discovery", "ownership": "RUST", "authority": "rust-mcp-catalog", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/mcp/oauth/check-status", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/mcp/oauth/login", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/mcp/oauth/logout", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/mcp/oauth/authenticated", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/auth/internal/users/system", "ownership": "RUST", "authority": "rust-compat", "auth": "public" }),
            json!({ "method": "POST", "path": "/api/auth/internal/users/system/credentials", "ownership": "RUST", "authority": "rust-compat", "auth": "public" }),
            json!({ "method": "GET", "path": "/api/route-ownership", "ownership": "RUST", "authority": "rust-compat", "auth": "public" }),
            json!({ "method": "POST", "path": "/api/shell/show-item-in-folder", "ownership": "LOCAL", "authority": "desktop-local", "auth": "desktop-session" }),
            json!({ "method": "POST", "path": "/api/shell/open-external", "ownership": "LOCAL", "authority": "desktop-local", "auth": "desktop-session" }),
            json!({ "method": "POST", "path": "/api/shell/check-tool-installed", "ownership": "LOCAL", "authority": "desktop-local", "auth": "desktop-session" }),
            json!({ "method": "POST", "path": "/api/shell/open-folder-with", "ownership": "LOCAL", "authority": "desktop-local", "auth": "desktop-session" }),
            json!({ "method": "POST", "path": "/api/providers", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/providers/fetch-models", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/providers/detect-protocol", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "PUT", "path": "/api/providers/{provider_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "DELETE", "path": "/api/providers/{provider_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/providers/{provider_id}/models", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/providers/{provider_id}/test", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/remote-agents", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/remote-agents", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/remote-agents/test-connection", "ownership": "RUST", "authority": "rust-compat-visible-degrade", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/remote-agents/{agent_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "PUT", "path": "/api/remote-agents/{agent_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "DELETE", "path": "/api/remote-agents/{agent_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/remote-agents/{agent_id}/handshake", "ownership": "RUST", "authority": "rust-compat-visible-degrade", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/conversations/clone", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/conversations/active-count", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/messages/search", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/conversations/{conversation_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "PATCH", "path": "/api/conversations/{conversation_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "DELETE", "path": "/api/conversations/{conversation_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/conversations/{conversation_id}/reset", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/conversations/{conversation_id}/associated", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/conversations/{conversation_id}/messages/{message_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/conversations/{conversation_id}/side-question", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/conversations/{conversation_id}/artifacts", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "PATCH", "path": "/api/conversations/{conversation_id}/artifacts/{artifact_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/conversations/{conversation_id}/approvals/check", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "PUT", "path": "/api/conversations/{conversation_id}/config-options/{option_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/conversations/{conversation_id}/openclaw/runtime", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/conversations/{conversation_id}/active-lease", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/conversations/{conversation_id}/slash-commands", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/teams/{team_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "DELETE", "path": "/api/teams/{team_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/teams/{team_id}/agents", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "DELETE", "path": "/api/teams/{team_id}/agents/{slot_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "PATCH", "path": "/api/teams/{team_id}/agents/{slot_id}/name", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "PATCH", "path": "/api/teams/{team_id}/name", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/teams/{team_id}/session", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "DELETE", "path": "/api/teams/{team_id}/session", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/teams/{team_id}/active-lease", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/teams/{team_id}/session-mode", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/teams/{team_id}/run-state", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/teams/{team_id}/agents/{slot_id}/messages", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/teams/{team_id}/runs/{team_run_id}/cancel", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/teams/{team_id}/runs/{team_run_id}/agents/{slot_id}/cancel", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/teams/{team_id}/runs/{team_run_id}/agents/{slot_id}/pause", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/v1/workflow-designs", "ownership": "RUST", "authority": "rust-enterprise-api", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/v1/workflow-designs", "ownership": "RUST", "authority": "rust-enterprise-api", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/v1/workflow-designs/{workflow_design_id}", "ownership": "RUST", "authority": "rust-enterprise-api", "auth": "bearer" }),
            json!({ "method": "PATCH", "path": "/api/v1/workflow-designs/{workflow_design_id}", "ownership": "RUST", "authority": "rust-enterprise-api", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/v1/workflow-designs/{workflow_design_id}/versions", "ownership": "RUST", "authority": "rust-enterprise-api", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/v1/workflow-designs/{workflow_design_id}/versions", "ownership": "RUST", "authority": "rust-enterprise-api", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/v1/workflow-versions/{workflow_version_id}", "ownership": "RUST", "authority": "rust-enterprise-api", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/v1/workflow-versions/{workflow_version_id}/validate", "ownership": "RUST", "authority": "rust-enterprise-api", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/v1/workflow-runs", "ownership": "RUST", "authority": "rust-enterprise-api", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/v1/workflow-runs", "ownership": "RUST", "authority": "rust-enterprise-api", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/v1/workflow-runs/{workflow_run_id}", "ownership": "RUST", "authority": "rust-enterprise-api", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/v1/workflow-runs/{workflow_run_id}/node-runs", "ownership": "RUST", "authority": "rust-enterprise-api", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/v1/workflow-runs/{workflow_run_id}/cancel", "ownership": "RUST", "authority": "rust-enterprise-api", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/v1/tool-result-artifacts/read", "ownership": "RUST", "authority": "rust-enterprise-api", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/v1/tool-result-artifacts/stream", "ownership": "RUST", "authority": "rust-enterprise-api", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/fs/fetch-remote-image", "ownership": "FACADE", "authority": "desktop-gateway-local|rust-file-service", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/fs/snapshot/info", "ownership": "LOCAL", "authority": "desktop-gateway-local-snapshot", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/fs/snapshot/dispose", "ownership": "LOCAL", "authority": "desktop-gateway-local-snapshot", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/fs/snapshot/stage", "ownership": "LOCAL", "authority": "desktop-gateway-local-snapshot", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/fs/snapshot/stage-all", "ownership": "LOCAL", "authority": "desktop-gateway-local-snapshot", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/fs/snapshot/unstage", "ownership": "LOCAL", "authority": "desktop-gateway-local-snapshot", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/fs/snapshot/unstage-all", "ownership": "LOCAL", "authority": "desktop-gateway-local-snapshot", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/fs/snapshot/discard", "ownership": "LOCAL", "authority": "desktop-gateway-local-snapshot", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/fs/snapshot/branches", "ownership": "LOCAL", "authority": "desktop-gateway-local-snapshot", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/cron/jobs/{job_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "PUT", "path": "/api/cron/jobs/{job_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "DELETE", "path": "/api/cron/jobs/{job_id}", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/cron/jobs/{job_id}/conversations", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "GET", "path": "/api/cron/jobs/{job_id}/skill", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "POST", "path": "/api/cron/jobs/{job_id}/skill", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
            json!({ "method": "DELETE", "path": "/api/cron/jobs/{job_id}/skill", "ownership": "RUST", "authority": "rust-compat", "auth": "bearer" }),
        ]);
    json!({
        "auth": prefixes["auth"],
        "me": prefixes["me"],
        "settings": prefixes["settings"],
        "routeOwnership": prefixes["routeOwnership"],
        "assistants": prefixes["assistants"],
        "agents.management": prefixes["agents.management"],
        "agents.custom": prefixes["agents.custom"],
        "skills": prefixes["skills"],
        "mcp": prefixes["mcp"],
        "providers": prefixes["providers"],
        "remoteAgents": prefixes["remoteAgents"],
        "conversations": prefixes["conversations"],
        "messages": prefixes["messages"],
        "teams": prefixes["teams"],
        "workflow": prefixes["workflow"],
        "fs": prefixes["fs"],
        "cron": prefixes["cron"],
        "channel.plugins": prefixes["channel.plugins"],
        "channel.facts": prefixes["channel.facts"],
        "extensions": prefixes["extensions"],
        "hub": prefixes["hub"],
        "shell": prefixes["shell"],
        "officePreview": prefixes["officePreview"],
        "stt": prefixes["stt"],
        "ws": prefixes["ws"],
        "prefixes": prefixes,
        "routes": routes,
    })
}

pub async fn biwork_list_route_ownership() -> Result<Json<Value>, AppError> {
    Ok(ok(route_ownership_manifest()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route_entry<'a>(manifest: &'a Value, method: &str, path: &str) -> &'a Value {
        manifest["routes"]
            .as_array()
            .expect("routes array")
            .iter()
            .find(|entry| entry["method"] == method && entry["path"] == path)
            .unwrap_or_else(|| panic!("missing route ownership entry: {method} {path}"))
    }
    fn compat_router_registration_pattern(manifest_path: &str) -> Option<String> {
        if manifest_path == "/ws" {
            return None;
        }
        if let Some(router_path) = manifest_path.strip_prefix("/api/v1") {
            return Some(format!("\"{router_path}\""));
        }
        let router_path = manifest_path
            .strip_prefix("/api")
            .unwrap_or_else(|| panic!("unexpected non-/api manifest route: {manifest_path}"));
        Some(format!("\"{router_path}\""))
    }

    fn compat_router_method_pattern(method: &str) -> Option<&'static str> {
        match method {
            "GET" => Some("get("),
            "POST" => Some("post("),
            "PUT" => Some("put("),
            "PATCH" => Some("patch("),
            "DELETE" => Some("delete("),
            "WS" => None,
            other => panic!("unsupported manifest method: {other}"),
        }
    }

    fn compat_router_route_block<'a>(
        router_source: &'a str,
        path_pattern: &str,
    ) -> Option<&'a str> {
        let path_start = router_source.find(path_pattern)?;
        let tail = &router_source[path_start..];
        let end = tail[path_pattern.len()..]
            .find("\n        .route(")
            .map(|offset| path_pattern.len() + offset)
            .unwrap_or(tail.len());
        Some(&tail[..end])
    }

    #[test]
    fn route_ownership_manifest_preserves_prefix_contract() {
        let manifest = route_ownership_manifest();

        assert_eq!(manifest["prefixes"]["conversations"], "RUST");
        assert_eq!(manifest["prefixes"]["agents"], "RUST");
        assert_eq!(manifest["prefixes"]["agents.custom"], "AGGREGATE");
        assert_eq!(manifest["prefixes"]["workflow"], "RUST");
        assert_eq!(manifest["prefixes"]["system"], "RUST");
        assert_eq!(manifest["prefixes"]["webui"], "RUST");
        assert_eq!(manifest["prefixes"]["google"], "RUST");
        assert_eq!(manifest["prefixes"]["bedrock"], "RUST");
        assert_eq!(manifest["prefixes"]["fs"], "FACADE");
        assert_eq!(manifest["prefixes"]["shell"], "LOCAL");
        assert_eq!(manifest["prefixes"]["extensions"], "AGGREGATE");
        assert_eq!(manifest["prefixes"]["ws"], "AGGREGATE");
        assert_eq!(manifest["fs"], "FACADE");
    }

    #[test]
    fn route_ownership_manifest_paths_are_registered_in_biwork_compat_router() {
        let manifest = route_ownership_manifest();
        let router_source = include_str!("../mod.rs");
        let missing = manifest["routes"]
            .as_array()
            .expect("routes array")
            .iter()
            .filter_map(|entry| {
                let method = entry["method"].as_str().expect("manifest method");
                let path = entry["path"].as_str().expect("manifest path");
                compat_router_registration_pattern(path).and_then(|pattern| {
                    let Some(route_block) = compat_router_route_block(router_source, &pattern)
                    else {
                        return Some(format!("{method} {path}"));
                    };
                    compat_router_method_pattern(method).and_then(|method_pattern| {
                        (!route_block.contains(method_pattern)).then(|| format!("{method} {path}"))
                    })
                })
            })
            .collect::<Vec<_>>();

        assert_eq!(missing, Vec::<String>::new());
    }
    #[test]
    fn route_ownership_manifest_covers_rust_owned_biwork_routes() {
        let manifest = route_ownership_manifest();

        let me = route_entry(&manifest, "GET", "/api/me");
        assert_eq!(me["ownership"], "RUST");
        assert_eq!(me["auth"], "bearer");

        let token_exchange = route_entry(&manifest, "POST", "/api/auth/oidc/token");
        assert_eq!(token_exchange["ownership"], "RUST");
        assert_eq!(token_exchange["auth"], "public");

        let bootstrap = route_entry(&manifest, "GET", "/api/workbench/bootstrap");
        assert_eq!(bootstrap["ownership"], "RUST");
        assert_eq!(bootstrap["auth"], "bearer");

        let ensure_node = route_entry(&manifest, "POST", "/api/system/ensure-node-runtime");
        assert_eq!(ensure_node["authority"], "rust-compat-visible-degrade");

        let ensure_acp = route_entry(&manifest, "POST", "/api/system/ensure-managed-acp-tool");
        assert_eq!(ensure_acp["ownership"], "RUST");

        let google_subscription = route_entry(&manifest, "GET", "/api/google/subscription-status");
        assert_eq!(google_subscription["ownership"], "RUST");

        let bedrock_test = route_entry(&manifest, "POST", "/api/bedrock/test-connection");
        assert_eq!(bedrock_test["authority"], "rust-compat-visible-degrade");

        let webui_username = route_entry(&manifest, "POST", "/api/webui/change-username");
        assert_eq!(webui_username["ownership"], "RUST");

        let webui_password = route_entry(&manifest, "POST", "/api/webui/change-password");
        assert_eq!(webui_password["authority"], "rust-compat-oidc");
        assert_eq!(webui_password["auth"], "bearer");

        let webui_reset = route_entry(&manifest, "POST", "/api/webui/reset-password");
        assert_eq!(webui_reset["authority"], "rust-compat-oidc");
        assert_eq!(webui_reset["auth"], "bearer");

        let webui_qr = route_entry(&manifest, "POST", "/api/webui/generate-qr-token");
        assert_eq!(webui_qr["authority"], "rust-compat-oidc");
        assert_eq!(webui_qr["auth"], "bearer");

        let assistant_create = route_entry(&manifest, "POST", "/api/assistants");
        assert_eq!(assistant_create["ownership"], "RUST");

        let assistant_get = route_entry(&manifest, "GET", "/api/assistants/{assistant_id}");
        assert_eq!(assistant_get["ownership"], "RUST");

        let assistant_update = route_entry(&manifest, "PUT", "/api/assistants/{assistant_id}");
        assert_eq!(assistant_update["authority"], "rust-compat");

        let assistant_state =
            route_entry(&manifest, "PATCH", "/api/assistants/{assistant_id}/state");
        assert_eq!(assistant_state["ownership"], "RUST");

        let agent_refresh = route_entry(&manifest, "POST", "/api/agents/refresh");
        assert_eq!(agent_refresh["ownership"], "RUST");

        let agent_overrides = route_entry(&manifest, "PUT", "/api/agents/{agent_id}/overrides");
        assert_eq!(agent_overrides["authority"], "rust-compat");

        let custom_agent_update = route_entry(&manifest, "PUT", "/api/agents/custom/{agent_id}");
        assert_eq!(custom_agent_update["ownership"], "AGGREGATE");

        let skill_import = route_entry(&manifest, "POST", "/api/skills/import");
        assert_eq!(skill_import["ownership"], "RUST");
        assert_eq!(skill_import["authority"], "rust-compat");

        let skill_create = route_entry(&manifest, "POST", "/api/skills");
        assert_eq!(skill_create["ownership"], "RUST");

        let builtin_rule = route_entry(&manifest, "POST", "/api/skills/builtin-rule");
        assert_eq!(builtin_rule["authority"], "rust-compat");

        let builtin_skill = route_entry(&manifest, "POST", "/api/skills/builtin-skill");
        assert_eq!(builtin_skill["ownership"], "RUST");

        let assistant_rule_write =
            route_entry(&manifest, "POST", "/api/skills/assistant-rule/write");
        assert_eq!(assistant_rule_write["ownership"], "RUST");

        let assistant_rule_delete = route_entry(
            &manifest,
            "DELETE",
            "/api/skills/assistant-rule/{assistant_id}",
        );
        assert_eq!(assistant_rule_delete["authority"], "rust-compat");

        let skill_history = route_entry(&manifest, "GET", "/api/skills/import-history");
        assert_eq!(skill_history["ownership"], "RUST");

        let skill_delete = route_entry(&manifest, "DELETE", "/api/skills/{skill_name}");
        assert_eq!(skill_delete["ownership"], "RUST");

        let skill_detect_external = route_entry(&manifest, "GET", "/api/skills/detect-external");
        assert_eq!(skill_detect_external["ownership"], "RUST");

        let skill_external_paths = route_entry(&manifest, "POST", "/api/skills/external-paths");
        assert_eq!(skill_external_paths["ownership"], "RUST");

        let skills_market_enable = route_entry(&manifest, "POST", "/api/skills/market/enable");
        assert_eq!(skills_market_enable["authority"], "rust-compat");

        let mcp_create = route_entry(&manifest, "POST", "/api/mcp/servers");
        assert_eq!(mcp_create["ownership"], "RUST");

        let mcp_update = route_entry(&manifest, "PUT", "/api/mcp/servers/{server_id}");
        assert_eq!(mcp_update["authority"], "rust-compat");

        let mcp_test = route_entry(&manifest, "POST", "/api/mcp/test-connection");
        assert_eq!(mcp_test["ownership"], "FACADE");
        assert_eq!(mcp_test["authority"], "desktop-stdio-mcp|rust-mcp-catalog");

        let mcp_oauth_login = route_entry(&manifest, "POST", "/api/mcp/oauth/login");
        assert_eq!(mcp_oauth_login["authority"], "rust-compat");

        let route_ownership = route_entry(&manifest, "GET", "/api/route-ownership");
        assert_eq!(route_ownership["auth"], "public");

        let provider_create = route_entry(&manifest, "POST", "/api/providers");
        assert_eq!(provider_create["ownership"], "RUST");

        let provider_fetch_models = route_entry(&manifest, "POST", "/api/providers/fetch-models");
        assert_eq!(provider_fetch_models["authority"], "rust-compat");

        let provider_detect = route_entry(&manifest, "POST", "/api/providers/detect-protocol");
        assert_eq!(provider_detect["authority"], "rust-compat");

        let provider_test = route_entry(&manifest, "POST", "/api/providers/{provider_id}/test");
        assert_eq!(provider_test["authority"], "rust-compat");

        let remote_agent_create = route_entry(&manifest, "POST", "/api/remote-agents");
        assert_eq!(remote_agent_create["ownership"], "RUST");

        let remote_agent_handshake =
            route_entry(&manifest, "POST", "/api/remote-agents/{agent_id}/handshake");
        assert_eq!(
            remote_agent_handshake["authority"],
            "rust-compat-visible-degrade"
        );

        let clone_conversation = route_entry(&manifest, "POST", "/api/conversations/clone");
        assert_eq!(clone_conversation["ownership"], "RUST");

        let search_messages = route_entry(&manifest, "GET", "/api/messages/search");
        assert_eq!(search_messages["ownership"], "RUST");

        let send_message = route_entry(
            &manifest,
            "POST",
            "/api/conversations/{conversation_id}/messages",
        );
        assert_eq!(send_message["ownership"], "RUST");
        assert_eq!(send_message["authority"], "rust-compat");

        let conversation_artifact = route_entry(
            &manifest,
            "PATCH",
            "/api/conversations/{conversation_id}/artifacts/{artifact_id}",
        );
        assert_eq!(conversation_artifact["ownership"], "RUST");

        let ensure_team_session = route_entry(&manifest, "POST", "/api/teams/{team_id}/session");
        assert_eq!(ensure_team_session["authority"], "rust-compat");

        let stop_team_session = route_entry(&manifest, "DELETE", "/api/teams/{team_id}/session");
        assert_eq!(stop_team_session["authority"], "rust-compat");

        let team_active_lease = route_entry(&manifest, "POST", "/api/teams/{team_id}/active-lease");
        assert_eq!(team_active_lease["authority"], "rust-compat");

        let team_agent_pause = route_entry(
            &manifest,
            "POST",
            "/api/teams/{team_id}/runs/{team_run_id}/agents/{slot_id}/pause",
        );
        assert_eq!(team_agent_pause["ownership"], "RUST");

        let workflow_run_create = route_entry(&manifest, "POST", "/api/v1/workflow-runs");
        assert_eq!(workflow_run_create["ownership"], "RUST");
        assert_eq!(workflow_run_create["authority"], "rust-enterprise-api");
        assert_eq!(workflow_run_create["auth"], "bearer");

        let workflow_run_cancel = route_entry(
            &manifest,
            "POST",
            "/api/v1/workflow-runs/{workflow_run_id}/cancel",
        );
        assert_eq!(workflow_run_cancel["ownership"], "RUST");
        assert_eq!(workflow_run_cancel["authority"], "rust-enterprise-api");

        let cron_job_update = route_entry(&manifest, "PUT", "/api/cron/jobs/{job_id}");
        assert_eq!(cron_job_update["ownership"], "RUST");

        let cron_job_skill = route_entry(&manifest, "GET", "/api/cron/jobs/{job_id}/skill");
        assert_eq!(cron_job_skill["ownership"], "RUST");

        let cron_resume = route_entry(&manifest, "POST", "/api/cron/internal/system-resume");
        assert_eq!(cron_resume["ownership"], "RUST");
        assert_eq!(cron_resume["auth"], "bearer");

        let approve_pairing = route_entry(&manifest, "POST", "/api/channel/pairings/approve");
        assert_eq!(approve_pairing["ownership"], "RUST");
        assert_eq!(approve_pairing["authority"], "rust-compat");

        let reject_pairing = route_entry(&manifest, "POST", "/api/channel/pairings/reject");
        assert_eq!(reject_pairing["ownership"], "RUST");

        let revoke_user = route_entry(&manifest, "POST", "/api/channel/users/revoke");
        assert_eq!(revoke_user["ownership"], "RUST");

        let set_assistant = route_entry(
            &manifest,
            "PUT",
            "/api/channel/settings/{platform}/assistant",
        );
        assert_eq!(set_assistant["ownership"], "RUST");

        let set_default_model = route_entry(
            &manifest,
            "PUT",
            "/api/channel/settings/{platform}/default-model",
        );
        assert_eq!(set_default_model["ownership"], "RUST");

        let sync_settings = route_entry(&manifest, "POST", "/api/channel/settings/sync");
        assert_eq!(sync_settings["ownership"], "RUST");
    }

    #[test]
    fn route_ownership_manifest_covers_local_and_facade_routes() {
        let manifest = route_ownership_manifest();

        let upload = route_entry(&manifest, "POST", "/api/fs/upload");
        assert_eq!(upload["ownership"], "FACADE");
        assert_eq!(upload["auth"], "bearer");

        let remote_image = route_entry(&manifest, "POST", "/api/fs/fetch-remote-image");
        assert_eq!(remote_image["ownership"], "FACADE");
        assert_eq!(
            remote_image["authority"],
            "desktop-gateway-local|rust-file-service"
        );

        let snapshot = route_entry(&manifest, "POST", "/api/fs/snapshot/compare");
        assert_eq!(snapshot["ownership"], "LOCAL");
        assert_eq!(snapshot["authority"], "desktop-gateway-local-snapshot");

        let snapshot_stage = route_entry(&manifest, "POST", "/api/fs/snapshot/stage-all");
        assert_eq!(snapshot_stage["ownership"], "LOCAL");
        assert_eq!(
            snapshot_stage["authority"],
            "desktop-gateway-local-snapshot"
        );

        let zip = route_entry(&manifest, "POST", "/api/fs/zip");
        assert_eq!(zip["ownership"], "LOCAL");
        assert_eq!(zip["authority"], "desktop-gateway-local-zip");

        let watch = route_entry(&manifest, "POST", "/api/fs/office-watch/start");
        assert_eq!(watch["ownership"], "LOCAL");
        assert_eq!(watch["authority"], "desktop-gateway-local-watch");

        let preview_history = route_entry(&manifest, "POST", "/api/preview-history/save");
        assert_eq!(preview_history["ownership"], "LOCAL");
        assert_eq!(
            preview_history["authority"],
            "desktop-gateway-local-preview-history"
        );

        let channel_plugins = route_entry(&manifest, "GET", "/api/channel/plugins");
        assert_eq!(channel_plugins["ownership"], "AGGREGATE");
        assert_eq!(
            channel_plugins["authority"],
            "desktop-gateway-local-channel-plugins+rust-governance"
        );

        let channel_plugin_test = route_entry(&manifest, "POST", "/api/channel/plugins/test");
        assert_eq!(channel_plugin_test["ownership"], "LOCAL");
        assert_eq!(
            channel_plugin_test["authority"],
            "desktop-gateway-local-channel-plugins"
        );
        assert_eq!(channel_plugin_test["auth"], "bearer");

        let channel_plugin_enable = route_entry(&manifest, "POST", "/api/channel/plugins/enable");
        assert_eq!(channel_plugin_enable["ownership"], "RUST");
        assert_eq!(channel_plugin_enable["authority"], "rust-compat");

        let channel_plugin_disable = route_entry(&manifest, "POST", "/api/channel/plugins/disable");
        assert_eq!(channel_plugin_disable["ownership"], "RUST");
        assert_eq!(channel_plugin_disable["authority"], "rust-compat");

        let hub_extensions = route_entry(&manifest, "GET", "/api/hub/extensions");
        assert_eq!(hub_extensions["ownership"], "AGGREGATE");
        assert_eq!(
            hub_extensions["authority"],
            "desktop-gateway-local-hub-index+rust-governance"
        );

        let hub_update = route_entry(&manifest, "POST", "/api/hub/update");
        assert_eq!(hub_update["ownership"], "LOCAL");
        assert_eq!(hub_update["authority"], "desktop-gateway-local-hub");
        assert_eq!(hub_update["auth"], "bearer");

        let hub_install = route_entry(&manifest, "POST", "/api/hub/install");
        assert_eq!(hub_install["ownership"], "LOCAL");
        assert_eq!(hub_install["auth"], "bearer");

        let hub_check_updates = route_entry(&manifest, "POST", "/api/hub/check-updates");
        assert_eq!(hub_check_updates["ownership"], "LOCAL");
        assert_eq!(hub_check_updates["auth"], "bearer");

        let extension_tabs = route_entry(&manifest, "GET", "/api/extensions/settings-tabs");
        assert_eq!(extension_tabs["ownership"], "AGGREGATE");
        assert_eq!(
            extension_tabs["authority"],
            "desktop-gateway-local-extensions+rust-governance"
        );

        let extension_channel_plugins =
            route_entry(&manifest, "GET", "/api/extensions/channel-plugins");
        assert_eq!(extension_channel_plugins["ownership"], "AGGREGATE");
        assert_eq!(
            extension_channel_plugins["authority"],
            "desktop-gateway-local-extensions+rust-governance"
        );

        let extension_agent_activity =
            route_entry(&manifest, "GET", "/api/extensions/agent-activity");
        assert_eq!(extension_agent_activity["ownership"], "AGGREGATE");
        assert_eq!(
            extension_agent_activity["authority"],
            "desktop-gateway-local-extensions+rust-governance"
        );

        let extension_sync = route_entry(&manifest, "POST", "/api/extensions/sync");
        assert_eq!(extension_sync["ownership"], "AGGREGATE");
        assert_eq!(
            extension_sync["authority"],
            "desktop-gateway-local-extensions+rust-governance"
        );

        let extension_disable = route_entry(&manifest, "POST", "/api/extensions/disable");
        assert_eq!(extension_disable["ownership"], "AGGREGATE");
        assert_eq!(
            extension_disable["authority"],
            "desktop-gateway-local-extensions+rust-governance"
        );

        let extension_static = route_entry(
            &manifest,
            "GET",
            "/api/extensions/static/{extension_name}/{*asset_path}",
        );
        assert_eq!(extension_static["ownership"], "LOCAL");
        assert_eq!(
            extension_static["authority"],
            "desktop-gateway-local-extension-static+rust-governance"
        );

        let shell = route_entry(&manifest, "POST", "/api/shell/open-file");
        assert_eq!(shell["ownership"], "LOCAL");
        assert_eq!(shell["authority"], "desktop-local");

        let shell_tool = route_entry(&manifest, "POST", "/api/shell/check-tool-installed");
        assert_eq!(shell_tool["ownership"], "LOCAL");
        assert_eq!(shell_tool["auth"], "desktop-session");

        let document_convert = route_entry(&manifest, "POST", "/api/document/convert");
        assert_eq!(document_convert["ownership"], "LOCAL");
        assert_eq!(document_convert["auth"], "desktop-session");

        let word_stop = route_entry(&manifest, "POST", "/api/word-preview/stop");
        assert_eq!(word_stop["ownership"], "LOCAL");
        assert_eq!(word_stop["authority"], "desktop-local");

        let ppt_proxy = route_entry(&manifest, "GET", "/api/ppt-proxy/{port}");
        assert_eq!(ppt_proxy["ownership"], "LOCAL");
        assert_eq!(ppt_proxy["authority"], "desktop-local-office-proxy");

        let office_proxy = route_entry(
            &manifest,
            "GET",
            "/api/office-watch-proxy/{port}/{*asset_path}",
        );
        assert_eq!(office_proxy["ownership"], "LOCAL");
        assert_eq!(office_proxy["auth"], "desktop-session");

        let stt = route_entry(&manifest, "POST", "/api/stt");
        assert_eq!(stt["ownership"], "RUST");
        assert_eq!(stt["authority"], "rust-compat-visible-degrade");
        assert_eq!(stt["auth"], "bearer");
        let stt_stream = route_entry(&manifest, "GET", "/api/stt/stream");
        assert_eq!(stt_stream["ownership"], "RUST");
        assert_eq!(stt_stream["authority"], "rust-compat-visible-degrade");
        assert_eq!(stt_stream["auth"], "bearer");

        let ws = route_entry(&manifest, "WS", "/ws");
        assert_eq!(ws["ownership"], "AGGREGATE");
        assert_eq!(ws["auth"], "ws-auth-frame");
    }
}
