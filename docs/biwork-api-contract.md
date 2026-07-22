# BiWork API Contract

更新日期：2026-07-14

## 1. 目的

本文档定义 BiWork renderer-facing `/api/*` 和 `/ws` 合同。

它只回答三件事：

1. 每个路由和事件归谁负责。
2. 前端真正依赖的最小 payload 形状是什么。
3. 重构后如何保证 BiWork 页面不因为接口漂移而失效。

本文件是 `docs/biwork-enterprise-agent-platform-execution-plan.md` 的配套合同文档。执行方案描述架构与步骤；本文件描述接口和事件的最低兼容面。

## 2. 合同层次

本轮合同分三层：

### 2.1 Enterprise Compat

由 Rust 提供企业资源事实和 compat/BFF 映射：

- auth
- settings
- assistants
- providers
- managed agents metadata
- conversations / runs / approvals
- workbench / file governance
- teams
- cron
- channel enterprise facts

### 2.2 Desktop Local

由 BiWork desktop gateway 提供桌面本地能力：

- shell
- Office preview
- preview history
- extension runtime
- hub install manager
- channel connector local runtime

### 2.3 Desktop Aggregate

由 desktop gateway 聚合 enterprise 与 local 两侧状态：

- `/ws`
- `/api/extensions/*`
- `/api/channel/plugins`
- `/api/hub/extensions`
- `/api/agents/custom*`

## 3. Ownership Classes

| 标记 | 含义 |
| --- | --- |
| `RUST` | 路由最终由 Rust enterprise compat 负责。 |
| `LOCAL` | 路由最终由 desktop local capability plane 负责。 |
| `AGGREGATE` | desktop gateway 负责聚合本地状态和 Rust 状态。 |
| `FACADE` | desktop gateway 做轻包装，最终仍调用 Rust enterprise service。 |

## 4. 通用规则

### 4.1 认证

除明确标注的本地便利能力外，所有 enterprise 和 aggregate 路由都必须接受：

```http
Authorization: Bearer <access_token>
```

access token 来源：

- renderer `httpBridge.ts`
- renderer `configService.ts`
- renderer `configMigration.ts`
- Electron main process helper
- desktop gateway WS bridge

以上调用方必须共享同一 token broker。

禁止新增绕过 token broker 的 renderer-facing enterprise/aggregate 通道：

- 不允许用原生 `EventSource` 访问 `/api/*` enterprise/aggregate 路由；如需要 SSE，必须使用可注入 `Authorization` 的 fetch-stream，或将路由显式定义为 `LOCAL`。
- 不允许新增并行的通用 `/ws` 客户端；renderer 必须通过共享 bridge，在 access token 可用后建连，并以 `auth` 作为首帧。

#### Shared Token Broker 设计

实现入口固定为 `bibi_work_frontend/packages/desktop/src/common/auth/authTokenBroker.ts`。

设计约束：

1. access token 只保存在内存 broker 中；禁止写入 `localStorage`、`sessionStorage` 或明文配置文件。
2. Electron 桌面端的 refresh token 只允许由 main process 持有，并使用系统安全存储加密落盘；renderer、preload 参数、localStorage、sessionStorage 和普通配置文件不得接触 refresh token。系统安全存储不可用时只能退化为进程内存保存，不允许明文持久化。
3. renderer 通过只读 `getAuthAccessToken(forceRefresh?)` 获取 main process 管理的 access token。生产 preload 不暴露 access token 写接口；`setAuthAccessToken` 仅允许在 `BIWORK_E2E_TEST=1` 的测试进程中注入测试凭证。
4. HTTP 调用统一用 `getAuthorizationHeaders()`；需要等待登录态的启动路径用 `getAccessToken()`；不能阻塞未登录启动的路径只允许用 `peekAccessToken()` 判定是否跳过。普通请求收到 401 后必须 single-flight 刷新并最多重试一次；第二次仍为 401 时清除完整桌面会话。
5. `configService.initialize()` 在没有 token 时必须快速完成并清空 session cache；token 后到达时必须重新同步 `/api/settings/client`。
6. `/ws` 必须等待 broker token 后建连，首帧发送 `{ "op": "auth", "access_token": "..." }`，之后再发送 subscribe；服务端会在连接存活期间周期重校验 session/device，撤销或过期时发送 `auth.revoked` 并关闭连接；收到当前 socket 针对当前 token 的 `auth.revoked` 或 `auth.failed` 后必须 `clearAccessToken()` 并禁止自动重连。已被替换的旧 socket 即使迟到发送 terminal auth 事件，也只能关闭自身，不能清空 replacement token 或关闭新 socket。
7. `/api/v1/conversations/{conversation_id}/events/stream` 这类 fetch-stream SSE 同样必须带 broker token；服务端每 5 秒重校验 session/device，撤销或过期时发送 SSE event `auth.revoked` 后结束流，客户端不得用原生 `EventSource` 绕过 Authorization。
8. `GET /api/stt/stream` 是 WebSocket endpoint，浏览器 API 不能附加 `Authorization` header；desktop/WebUI 客户端必须在 start frame 中携带 broker token 或依赖 WebUI same-origin session cookie。
9. 主动登出必须先撤销平台 session，再尽力撤销 OIDC refresh token，最后无条件清除 main process 安全存储、renderer access token 和 token-scoped cache。refresh 返回 `invalid_grant`、第二次 401 或 session revoke 时同样清除完整会话。

### 4.2 响应 envelope

默认成功响应：

```json
{
  "success": true,
  "data": {},
  "trace_id": "01HZ..."
}
```

默认失败响应：

```json
{
  "success": false,
  "error": "permission denied",
  "code": "PERMISSION_DENIED",
  "details": {},
  "trace_id": "01HZ..."
}
```

说明：

1. `httpBridge.ts` 会自动解包 `data` 字段。
2. `code`、`error`、`details` 必须可供 `BackendHttpError` 读取。
3. `trace_id` 推荐始终返回，便于截图和日志对齐。

### 4.3 特例 envelope

`GET /api/auth/user` 是遗留兼容例外，必须返回：

```json
{
  "success": true,
  "user": {
    "id": "user_123",
    "username": "alice"
  }
}
```

原因：`AuthContext.tsx` 目前直接读取 `success + user`。

### 4.4 时间字段

Compat `/api/*` 的时间字段应优先使用 epoch milliseconds number，而不是 RFC3339 string。

原因：

- `TChatConversation.created_at/modified_at` 是 number
- `Assistant.last_used_at` 是 number
- `ICronJob.metadata.created_at/updated_at` 是 number
- `IMcpServer.created_at/updated_at` 是 number
- `message.stream.created_at` 是 number

Enterprise `/api/v1/*` 可以继续使用自己的 `ResourceResponse` 约定；但 compat `/api/*` 应按 BiWork 现有类型返回 number，减少 mapper 和页面改造成本。

### 4.5 模型字段

BiWork conversation create/update 使用的 model 形状不是 `IProvider` 全量，而是：

```json
{
  "provider_id": "provider_openai_1",
  "model": "gpt-4.1",
  "use_model": "gpt-4.1"
}
```

说明：

1. 前端通过 `toApiModelOptional()` 发送该形状。
2. 返回给前端时，compat 层可以继续返回该最小形状，BiWork 会用 `fromApiConversation()` 转为 `TProviderWithModel` 的轻量对象。

### 4.6 错误码最低集合

本轮 compat 至少要稳定以下错误码：

```text
UNAUTHORIZED
FORBIDDEN
NOT_FOUND
VALIDATION_ERROR
CONFLICT
CONVERSATION_ARCHIVED
RUNTIME_NOT_ENABLED
FEATURE_NOT_AVAILABLE
OIDC_REQUIRED
CHANNEL_PLUGIN_DISABLED
EXTENSION_NOT_ALLOWED
WORKSPACE_REVISION_CONFLICT
APPROVAL_REQUIRED
```

## 5. Route Ownership Manifest

### 5.1 Prefix-level ownership

| 路由前缀 | Ownership | 说明 |
| --- | --- | --- |
| `/api/auth/*` | `RUST` | OIDC/session projection |
| `/api/me` | `RUST` | 当前用户上下文 |
| `/api/settings*` | `RUST` | `user_ui_preferences` |
| `/api/system/*` | `RUST` | system info and visible runtime degrade |
| `/api/webui/*` | `RUST` | WebUI credential compatibility on OIDC backend |
| `/api/google/*` | `RUST` | Google subscription visible degrade |
| `/api/bedrock/*` | `RUST` | Bedrock provider compatibility visible degrade |
| `/api/route-ownership` | `RUST` | desktop gateway / backend route ownership manifest |
| `/api/assistants*` | `RUST` | assistant profile compat |
| `/api/agents/management`, `/api/agents/refresh`, `/api/agents/{id}/enabled`, `/api/agents/{id}/health-check`, `/api/agents/{id}/overrides`, `/api/agents/provider-health-check` | `RUST` | enterprise managed agent catalog and health projection |
| `/api/agents/custom*` | `AGGREGATE` | 本地 runtime 配置体验 + Rust governance |
| `/api/skills*` | `RUST` | skill catalog |
| `/api/mcp/*` | `RUST` | MCP catalog and discovery |
| `/api/providers*` | `RUST` | provider/profile/credential compat |
| `/api/remote-agents*` | `RUST` | remote agent metadata compat; live handshake/test visible-degrade while feature flags hide direct remote runtime |
| `/api/workbench/*` | `RUST` | Guid/workbench bootstrap and feature gating |
| `/api/conversations*` | `RUST` | conversation/run/approval/workbench mainline |
| `/api/messages/*` | `RUST` | cross-conversation message search |
| `/api/teams*` | `RUST` | team compat |
| `/api/v1/workflow-*` | `RUST` | enterprise Workflow DAG control plane passthrough; not team compat and not desktop local |
| `/api/v1/tool-result-artifacts/*` | `RUST` | enterprise artifact read/stream data plane for large tool results |
| `/api/fs/*` | `FACADE` | gateway facade -> Rust workbench/file service |
| `/api/cron/*` | `RUST` | scheduled jobs |
| `/api/channel/plugins` | `AGGREGATE` | local connector runtime + Rust policy/config projection |
| `/api/channel/plugins/test` | `LOCAL` | session-gated local connector dry-run, not proxied to Rust |
| `/api/channel/plugins/enable`, `/api/channel/plugins/disable` | `RUST` | enterprise enable/disable governance and audit |
| `/api/channel/ingress/*` | `RUST` | connector message ingress and run dispatch |
| `/api/channel/pairings*` | `RUST` | pairing facts |
| `/api/channel/users*` | `RUST` | authorized users |
| `/api/channel/sessions*` | `RUST` | channel session facts |
| `/api/channel/settings*` | `RUST` | assistant/default model binding |
| `/api/extensions*` | `AGGREGATE` | local extension runtime + Rust governance filter |
| `/api/hub/extensions` | `AGGREGATE` | local hub index + Rust governance projection |
| `/api/hub/install`, `/api/hub/uninstall`, `/api/hub/retry-install`, `/api/hub/check-updates`, `/api/hub/update` | `LOCAL` | hub install/update manager |
| `/api/shell/*` | `LOCAL` | desktop convenience actions |
| `/api/ppt-preview/*` | `LOCAL` | Office preview |
| `/api/word-preview/*` | `LOCAL` | Office preview |
| `/api/excel-preview/*` | `LOCAL` | Office preview |
| `/api/ppt-proxy/{port}*`, `/api/office-watch-proxy/{port}*` | `LOCAL` | Office watch iframe proxy to loopback OfficeCLI server |
| `/api/stt`, `/api/stt/stream` | `RUST` | speech-to-text visible-degrade until desktop speech runtime is attached |
| `/ws` | `AGGREGATE` | desktop WS multiplexer |

### 5.2 WebUI fallback

纯 WebUI 模式下：

1. `RUST` 路由保持可用。
2. `FACADE` 路由只有在 gateway 存在时可用。
3. `LOCAL` 路由必须：
   - 返回明确 `FEATURE_NOT_AVAILABLE`，
   - 或在 UI 层按 feature flag 隐藏。

## 6. Core REST Contracts

### 6.1 Auth

#### `GET /api/auth/oidc/config`

Owner: `RUST`

Response:

```json
{
  "success": true,
  "data": {
    "issuer": "https://id.example.com/realms/bibi-work",
    "client_id": "bibi-work-desktop",
    "authorization_endpoint": "https://id.example.com/realms/bibi-work/protocol/openid-connect/auth",
    "token_endpoint": "https://id.example.com/realms/bibi-work/protocol/openid-connect/token",
    "revocation_endpoint": "https://id.example.com/realms/bibi-work/protocol/openid-connect/revoke",
    "jwks_uri": "https://id.example.com/realms/bibi-work/protocol/openid-connect/certs",
    "scopes": ["openid", "profile", "email", "roles"],
    "desktop_callback": {
      "kind": "loopback",
      "redirect_uri": "http://127.0.0.1:48123/callback"
    },
    "web_callback": {
      "redirect_uri": "https://app.example.com/auth/callback"
    }
  }
}
```

#### `GET /api/auth/user`

Owner: `RUST`

Response:

```json
{
  "success": true,
  "user": {
    "id": "u_alice",
    "username": "alice"
  }
}
```

#### `POST /api/auth/oidc/token`

Owner: `RUST` (public OIDC exchange proxy)

支持 `authorization_code` + PKCE 和 `refresh_token` 两种 grant。桌面端 authorization request 必须包含 `offline_access`；refresh token 请求体只能从 Electron main process 发出。返回值透传 OIDC token response，renderer 不得调用 refresh grant。

#### `POST /api/auth/oidc/revoke`

Owner: `RUST` (public OIDC revocation proxy)

仅接受固定桌面 `client_id` 和 refresh token，由 Electron main process 在主动登出时调用。无论上游撤销是否成功，客户端都必须清除本地安全存储。

#### `POST /api/auth/logout`

Owner: `RUST`

Request body:

```json
{}
```

Response:

```json
{
  "success": true,
  "data": {
    "revoked": true
  }
}
```

#### `POST /api/auth/session/activity`

Owner: `RUST`

Electron renderer 只在已认证状态下，将 `pointerdown`、`keydown`、`touchstart`、滚动和窗口重新获得焦点等真实用户操作节流上报给 main process；main process 再调用本接口。后台 HTTP、WebSocket 心跳、Agent 输出和定时同步不得调用本接口。

桌面 session 默认连续 30 分钟没有用户活动即失效。每次成功调用会更新 `last_user_activity_at` 和 `idle_expires_at`；HTTP、WebSocket 与 SSE 对 desktop session 校验同一 `idle_expires_at`。WebUI session 不应用桌面空闲策略。

#### WebUI Credential Compatibility

Owner: `RUST`

Enterprise mode uses FerrisKey/OIDC for WebUI authentication. Desktop lifecycle code must not seed or reset local WebUI passwords from the main process.

Compatibility routes such as `/api/webui/change-password`, `/api/webui/reset-password`, `/api/webui/generate-qr-token`, and legacy `/api/auth/internal/users/system/credentials` are Rust routes and must return a visible unsupported response unless an OIDC-backed implementation is added. The reset-password CLI must probe `/api/auth/status` first and fail clearly when `auth_mode` is `ferriskey_oidc`.

The desktop WebUI status IPC must surface the backend `auth_mode` as `authMode`. When `authMode === "ferriskey_oidc"`, the settings UI must hide local password reset and QR-login controls and show an OIDC-managed credential notice instead.

### 6.2 Settings

#### `GET /api/settings/client?keys=...`

Owner: `RUST`

Response:

```json
{
  "success": true,
  "data": {
    "language": "zh-CN",
    "theme.activeId": "dark",
    "system.notificationEnabled": true
  }
}
```

#### `PUT /api/settings/client`

Owner: `RUST`

Request:

```json
{
  "language": "zh-CN",
  "system.notificationEnabled": true
}
```

Response:

```json
{
  "success": true,
  "data": {
    "updated": ["language", "system.notificationEnabled"]
  }
}
```

### 6.2.1 System / Provider Compatibility

Owner: `RUST`

Required endpoints:

- `GET /api/system/info`: 返回 `{ cache_dir, work_dir, log_dir, platform, arch }`。
- `POST /api/system/ensure-node-runtime`: 返回 `{ ready: false, code, message, scope }`，表达 Node runtime 准备属于 desktop local runtime；不能返回 404/503。
- `POST /api/system/ensure-managed-acp-tool`: 返回 `{ ready: false, code, message, tool_id, scope }`，表达 managed ACP tool 安装属于 desktop local runtime；不能返回 404/503。
- `GET /api/google/subscription-status`: 返回非订阅状态和 `lastChecked`，enterprise OIDC backend 不执行 Google OAuth 订阅检查。
- `POST /api/bedrock/test-connection`: 返回明确 `msg/code`，不伪造真实 AWS Bedrock 连接成功；真实运行校验走 enterprise provider/model profile test。

### 6.3 Assistants

#### `GET /api/assistants`

Owner: `RUST`

Contract:

- 返回数组
- 每项至少满足 `Assistant` 类型
- 返回集合必须按当前认证 actor 的 `run:agent:{agent_id}` 决策过滤；tenant member 不因同租户而默认看到所有 Agent。`GET /api/assistants/{id}`、conversation create/clone 也必须复用同一授权事实，禁止通过直接 ID 绕过列表过滤。
- `source` 必须归一化为 BiWork 前端实际支持的 `builtin | generated | user`；后端存储中的 `custom/remote/extension` 不能直接透传到 `Assistant.source`
- 可执行 runtime 只有两类，且执行边界不同：
  - `runtime.kind=deepagents`：由 Rust 创建 run snapshot，并分发给 `bibi_work_agent` 的 Python DeepAgents runtime。
  - `runtime.kind=biwork_cli`：由 Rust 写入 desktop local-exec 队列，再由 desktop gateway/本机 ACP runtime 执行；禁止分发给 Python runtime。
- `runtime.kind=acp` 是已废弃的旧值，不再兼容或自动映射为 `biwork_cli`；`remote`、`disabled` 及其他未知值只能作为不可运行的 catalog 项存在，不能投影为 `enabled=true` 或 `agent_status=online`。

Example:

```json
{
  "success": true,
  "data": [
    {
      "id": "assistant_general",
      "source": "builtin",
      "name": "General Assistant",
      "name_i18n": { "en-US": "General Assistant", "zh-CN": "通用助手" },
      "description": "General purpose enterprise assistant",
      "description_i18n": {},
      "avatar": "",
      "enabled": true,
      "sort_order": 10,
      "agent_id": "agent_general",
      "agent": {
        "type": "acp",
        "source": "internal",
        "acp_backend": "deepagents"
      },
      "enabled_skills": ["skill_summary"],
      "custom_skill_names": [],
      "disabled_builtin_skills": [],
      "context": "",
      "context_i18n": {},
      "prompts": [],
      "prompts_i18n": {},
      "models": ["provider_openai_1:gpt-4.1"],
      "last_used_at": 1751900000000,
      "agent_status": "online",
      "team_selectable": true,
      "deletable": false
    }
  ]
}
```

#### `GET /api/assistants/{id}`

Owner: `RUST`

Contract:

- 返回 `AssistantDetail`
- `profile/state/engine/rules/prompts/defaults/capabilities/preferences` 不能为空对象

Example:

```json
{
  "success": true,
  "data": {
    "id": "assistant_general",
    "source": "builtin",
    "agent_status": "online",
    "team_selectable": true,
    "deletable": false,
    "profile": {
      "name": "General Assistant",
      "name_i18n": {},
      "description": "General purpose enterprise assistant",
      "description_i18n": {},
      "avatar": ""
    },
    "state": {
      "enabled": true,
      "sort_order": 10,
      "last_used_at": 1751900000000
    },
    "engine": {
      "agent_id": "agent_general",
      "agent": {
        "type": "acp",
        "source": "internal",
        "acp_backend": "deepagents"
      }
    },
    "rules": {
      "content": "",
      "storage_mode": "inline"
    },
    "prompts": {
      "recommended": [],
      "recommended_i18n": {}
    },
    "defaults": {
      "model": { "mode": "inherit" },
      "permission": { "mode": "inherit" },
      "thought_level": { "mode": "inherit" },
      "skills": { "mode": "replace", "value": ["skill_summary"] },
      "mcps": { "mode": "replace", "value": [] }
    },
    "capabilities": {
      "default_skill_ids": ["skill_summary"],
      "custom_skill_names": [],
      "default_disabled_builtin_skill_ids": []
    },
    "preferences": {
      "last_skill_ids": [],
      "last_disabled_builtin_skill_ids": [],
      "last_mcp_ids": []
    }
  }
}
```

#### Assistant write endpoints

Owner: `RUST`

Required endpoints:

- `POST /api/assistants`: 创建 BiWork assistant profile，写入 enterprise `agents.draft_config/metadata` 并发布 `agent_versions` snapshot。
- `POST /api/assistants/import`: 批量导入/upsert BiWork assistant payload。
- `PUT /api/assistants/{assistant_id}`: 更新 profile、engine、capabilities、prompts、defaults。
- `PATCH /api/assistants/{assistant_id}/state`: 更新 `enabled/sort_order/last_used_at`。
- `DELETE /api/assistants/{assistant_id}`: soft delete non-builtin assistant；builtin assistant 不可删除。
- `POST /api/skills/assistant-rule/read`: 读取助手规则文本；优先返回 `context_i18n[locale]`，否则返回 `draft_config.system_prompt`。
- `POST /api/skills/assistant-rule/write`: 写入助手规则文本到 `agents.draft_config` 并发布新的 `agent_versions` snapshot；带 `locale` 时同步维护 `context_i18n[locale]`。
- `DELETE /api/skills/assistant-rule/{assistant_id}`: 清空助手规则文本和本地化规则，并发布新的 `agent_versions` snapshot。

### 6.4 Providers

#### `GET /api/providers`

Owner: `RUST`

Contract:

- 返回数组
- 每项兼容 `IProvider`
- `api_key` 字段对 compat 前端保留，但返回值必须为空字符串或脱敏占位，不得返回真实密钥

Example:

```json
{
  "success": true,
  "data": [
    {
      "id": "provider_openai_1",
      "platform": "openai-compatible",
      "name": "OpenAI Compatible",
      "base_url": "https://api.example.com/v1",
      "api_key": "",
      "models": ["gpt-4.1", "gpt-4o-mini"],
      "enabled": true,
      "model_enabled": {
        "gpt-4.1": true,
        "gpt-4o-mini": true
      }
    }
  ]
}
```

#### `POST /api/providers/fetch-models`

Owner: `RUST`

Request:

```json
{
  "platform": "openai-compatible",
  "base_url": "https://api.example.com/v1",
  "api_key": "sk-***",
  "try_fix": true
}
```

Response:

```json
{
  "success": true,
  "data": {
    "models": [
      "gpt-4.1",
      { "id": "gpt-4o-mini", "name": "gpt-4o-mini" }
    ]
  }
}
```

#### `POST /api/providers/{provider_id}/test`

Owner: `RUST`

Request:

```json
{
  "model": "gpt-5"
}
```

Response:

```json
{
  "success": true,
  "data": {
    "provider_id": "provider_openai_1",
    "platform": "openai-compatible",
    "model": "gpt-5",
    "status": "healthy",
    "elapsed_ms": 123,
    "message": "LLM provider connection succeeded and target model is available",
    "http_status": 200,
    "model_available": true,
    "checked_model_count": 12
  }
}
```

约束：

- BiWork renderer 只传 `provider_id` 和 `model`，不传 API key。
- Rust compat 根据 provider/model 找到 active `llm_model_profiles`，复用 enterprise `POST /api/v1/llm-model-profiles/{profile_id}/test`。
- Enterprise test 由 Rust 解析 active credential secret，调用 provider models endpoint，并确认目标 `model_name` 在返回模型列表中存在。

#### `POST /api/v1/llm-credentials/{credential_id}/rotate`

Owner: `RUST`

Request:

```json
{
  "tenant_id": "tenant_001",
  "secret_ref": "env://OPENAI_API_KEY_NEXT",
  "secret_hash": "sha256:...",
  "expires_at": "2026-12-31T00:00:00Z"
}
```

Response:

```json
{
  "id": "credential_001",
  "tenant_id": "tenant_001",
  "name": "credential 12345678",
  "description": "OpenAI",
  "status": "active",
  "metadata": {
    "provider_id": "provider_001",
    "has_secret_ref": true,
    "has_secret_hash": true,
    "last_rotated_at": "2026-07-10T00:00:00Z",
    "rotated_by_user_id": "user_001",
    "revoked_at": null
  },
  "created_at": "2026-07-01T00:00:00Z",
  "updated_at": "2026-07-10T00:00:00Z"
}
```

约束：

- 只允许 rotate active credential；revoked credential 不会被 rotate 复活。
- 响应不返回 `secret_ref` 或 secret value，只返回 `has_secret_ref/has_secret_hash/last_rotated_at/rotated_by_user_id` 等脱敏状态。
- `secret_ref` 不能为空白字符串，只允许 `env://NAME`、`vault://path#field` 或 `kms://key-id#ciphertext`；数据库和 Rust API 双重校验 scheme/path，响应始终不返回引用原文或 secret value。

#### Credential rotation automation

```text
POST /api/v1/llm-credentials/{credential_id}/rotation-policy
GET  /api/v1/llm-credential-rotation/health?tenant_id={tenant_id}
GET  /api/v1/llm-credential-rotation/attempts?tenant_id={tenant_id}&status={status}
```

- policy payload：`tenant_id`、`enabled`、启用时必填的 `interval_seconds`，以及可选 `rotate_before_seconds`。
- worker 或 rotation gateway 未配置时，启用策略返回 `409 CONFLICT`；关闭策略始终允许。
- health 返回 worker/gateway 状态、enabled/due/running/error 数量和最近 24 小时失败次数。
- attempt 只返回 resolver scheme、旧/新引用 hash、状态和有界错误，不返回原始 secret ref 或 secret value。
- BiWork 模型设置页只消费 health、脱敏 credential metadata 和 policy 更新；attempt hash 与错误正文不进入 renderer。

### 6.4.1 Audit governance

Owner: `RUST`

```text
GET  /api/v1/audit/hash-chain:verify
POST /api/v1/audit/hash-chain:seal
GET  /api/v1/audit/hash-chain:backfill-status
POST /api/v1/audit/hash-chain:backfill
GET  /api/v1/audit/legal-holds
POST /api/v1/audit/legal-holds
POST /api/v1/audit/legal-holds/{hold_id}/release
GET  /api/v1/audit/retention/eligibility
POST /api/v1/audit/retention/partitions:cleanup
```

约束：

- legal hold scope 仅允许 `tenant | segment | resource`；同 scope 同时只能存在一个 active hold。
- legal-hold reason 和 metadata 在 Rust 持久化前统一脱敏，renderer 不应发送或展示 secret value。
- retention eligibility 按整个 UTC 月度叶子分区报告安全资格；active hold、未到期、未验证归档、segment 跨分区、默认分区或任意未被合格 segment 覆盖的行都会返回明确 blocking reason。
- partition cleanup 默认 `dry_run=true`，只接受 `audit_logs_pYYYYMM` 历史分区，要求 FerrisKey `platform_admin`；服务端 `audit_partition.cleanup_enabled` 默认关闭。执行时 Rust 锁定父表并二次校验后才 detach/drop，renderer 不参与资格判断。
- backfill 默认 `dry_run=true`。只有 tenant 全部记录均未哈希且没有 sealed segment 时允许原地执行；混合链和已封存链返回 `requires_offline_rechain=true` 并拒绝执行。

### 6.5 Managed Agents

#### `GET /api/agents/management`

Owner: `RUST`

Contract:

- 返回数组
- 每项兼容 `ManagedAgent`
- 主要用于管理页和 Guid 运行时目录
- `agent_source` 必须归一化为 `internal | builtin | extension | custom`；remote 运行时使用 `agent_type: "remote"`，不能把 `remote` 放进 `agent_source`

Required minimum fields:

```json
{
  "id": "agent_general",
  "name": "General Agent",
  "agent_type": "acp",
  "agent_source": "internal",
  "enabled": true,
  "installed": true,
  "status": "online"
}
```

### 6.6 Skills Catalog

Owner: `RUST`

Required endpoints:

- `GET /api/skills`: 返回 BiWork `SkillInfo[]`，每项至少包含 `name/description/location/is_auto_inject/is_custom/source`。
- `POST /api/skills`: 创建或覆盖 tenant 内 custom skill，写入 enterprise `skills/skill_versions` catalog 并返回 BiWork `SkillInfo`。
- `POST /api/skills/builtin-rule|builtin-skill`: 从 enterprise catalog 读取最新 `skill_versions.manifest.content`；缺失时返回空字符串，不要求 desktop local builtin-skills runtime。
- `POST /api/skills/import`: 导入本地 `.md`/`SKILL.md`、目录、ZIP，GitHub repository/tree URL，或任意公网 HTTPS `SKILL.md`/ZIP 到 enterprise `skills/skill_versions` catalog；通用远程地址执行 SSRF、重定向和有界流式下载校验，持久化 source label 必须移除 query/fragment。失败项使用 BiWork Skill 导入错误码返回在 `failed[]`，并写入 import history。
- `POST /api/skills/import-upload`: multipart 上传 ZIP 并复用同一包解析、授权、catalog upsert 和 import history 逻辑；该入口适用于 WebUI，不依赖 Electron 与 Rust 共享本地路径。
- `POST /api/skills/info`: 读取本地 skill 文件/目录元信息，返回 `{ name, description }`。
- `POST /api/skills/scan`: 扫描目录下可导入 skill，返回 `{ name, description, path }[]`。
- `GET /api/skills/import-history`: 返回最近导入历史，字段兼容 BiWork `SkillImportRecord`。
- `GET /api/skills/import-limits`: 返回 `{ max_file_bytes, max_total_bytes }`。
- `GET /api/skills/paths`: 返回 enterprise 虚拟路径，不能要求 desktop local runtime。
- `GET|POST|DELETE /api/skills/external-paths`: Rust-owned 用户偏好接口，持久化 `{ name, path }[]` 到 `user_ui_preferences["skills.externalPaths"]`；这里只保存路径配置，不访问本地文件系统。
- `POST /api/skills/materialize-for-agent`: 将会话选择的 skill 名称解析为 enterprise catalog source path。
- `POST /api/skills/assistant-rule/read|write` 和 `DELETE /api/skills/assistant-rule/{assistant_id}`: Assistant rules 是 Rust-owned catalog 数据，不能落回 desktop local filesystem。
- `POST /api/skills/market/enable|disable`: Rust-owned 用户偏好接口，写入 `user_ui_preferences["skillsMarket.enabled"]`，不要求 desktop local runtime。
- `DELETE /api/skills/{skill_name}`: 删除 tenant 内 custom/imported skill；`builtin/extension/cron` 来源不可删除。

导入合同：

- Skill 包以包含 `SKILL.md` 的目录为标准格式；ZIP 可以包含多个 Skill，并允许 `references/`、`assets/` 等资源，但导入阶段不得隐式执行 `scripts/` 或其他代码。
- GitHub 来源必须固定到 commit；其他远程来源只接受公网 HTTPS 原始 `SKILL.md` 或 ZIP。普通 HTML 项目主页不是机器合同，不做页面抓取。
- 远程下载逐跳校验协议、DNS 和重定向，拒绝 URL credentials、本机、私网、链路本地和保留地址；持久化来源 URI 前移除 query/fragment。
- 本地 ZIP、远程 ZIP 和 multipart ZIP 共用同一内存解析器，拒绝 zip-slip、symlink、非法 UTF-8、重复名称、条目过多、单文件或解压总量超限。
- 默认上限为单文件 1 MiB、解压总量 10 MiB、2,048 个 ZIP 条目和 64 个 Skill；客户端必须以 `GET /api/skills/import-limits` 返回值为准，不硬编码限制。
- 所有入口返回统一的 `{ skill_name, skill_names, failed }` 结构，并写入同一导入历史。

### 6.7 MCP Catalog

Owner: `RUST`

Required endpoints:

- `GET /api/mcp/servers`: 返回 BiWork `IMcpServer[]`，每项至少包含 `id/name/description/enabled/transport/tools/original_json/builtin/created_at/updated_at`，以及脱敏的 `health_status/last_health_check/last_connected/consecutive_failures/has_health_error`。
- `POST /api/mcp/servers`: 将 BiWork `transport/original_json/builtin` payload 写入 enterprise `mcp_servers`。
- `POST /api/mcp/servers/import`: 批量 upsert BiWork MCP server payload。
- `PUT /api/mcp/servers/{server_id}`: 更新 tenant 内 MCP server。
- `DELETE /api/mcp/servers/{server_id}`: soft delete tenant 内 MCP server。
- `POST /api/mcp/servers/{server_id}/toggle`: 在 `active/disabled` 间切换。
- `POST /api/mcp/test-connection`: HTTP/SSE/streamable HTTP 使用 Rust MCP discovery 写回 `mcp_tools`，成功时把 `tools/list` 当作权威快照并停用未返回工具，失败时写结构化 unhealthy/连续失败状态且保留上一版工具。Rust 直连入口对 stdio 返回明确的 local-runtime-required failure。
- Electron 模式下 `POST /api/mcp/test-connection` 是 `FACADE`：HTTP/SSE/streamable HTTP 继续代理 Rust；stdio 由 Electron main 使用官方 MCP SDK 执行，并调用 `POST /api/mcp/servers/{server_id}/local-discovery` 回写脱敏结果。浏览器/WebUI 直连 Rust 时 stdio 仍返回 local-runtime-required。
- stdio `transport.env` 的值只允许 `env://NAME`，不接受明文环境变量值。
- Python runtime 调用 stdio MCP 时仍只访问 `POST /internal/mcp-tools:call`。Rust 使用数据库中的真实 tool/server/schema 做授权和风险判定，要求 actor `device_id`，并写入 device-bound `local_runtime.v1/mcp_stdio` work item；Electron main 通过 `GET /api/v1/local-exec/requests/next?...&kind=mcp_stdio` 领取并调用官方 SDK，随后事务性提交 terminal result。renderer 不参与执行，也不能提交任意 work item。
- `streamable-http` 的 Rust transport 会执行 initialize/initialized、保存并复用 `Mcp-Session-Id`、携带 negotiated protocol version，并从 JSON 或 `text/event-stream` 中提取匹配 JSON-RPC response。`sse` 是不同的 legacy negotiation transport，未实现时返回明确 unsupported，不伪装成普通 HTTP。
- `GET /api/mcp/agent-configs`: 返回可导入 agent MCP 配置列表；无本地 runtime 时为空数组。
- `POST /api/mcp/oauth/check-status`: Rust compat endpoint；校验 `server_url` 后返回 `{ authenticated: false }`，表示当前 Rust catalog 没有本地 OAuth token。
- `POST /api/mcp/oauth/login`: Rust compat endpoint；返回 `{ success: false, code: "MCP_OAUTH_LOCAL_RUNTIME_REQUIRED", error }`，不抛 HTTP 503。
- `POST /api/mcp/oauth/logout`: Rust compat endpoint；幂等清理语义，当前无本地 token 时返回成功空值。
- `GET /api/mcp/oauth/authenticated`: 返回已认证 server URL 列表；无本地 runtime 时为空数组。

### 6.8 Workbench Bootstrap

#### `GET /api/workbench/bootstrap`

Owner: `RUST`

Contract:

- 返回 Guid/Workbench 启动所需的 enterprise catalog 聚合。
- `feature_flags` 必须明确隐藏本轮不适配的 `remote_agent_direct`、`cdp_remote_control`、`local_remote_control`。
- `catalog` 下必须至少包含 `assistants/providers/skills/mcp_servers/managed_agents`。

Response minimum:

```json
{
  "success": true,
  "data": {
    "auth": {
      "tenant_id": "uuid",
      "user_id": "uuid",
      "roles": ["tenant_member"]
    },
    "runtime": {
      "default_kind": "deepagents",
      "supported_kinds": ["deepagents", "biwork_cli", "disabled"]
    },
    "feature_flags": {
      "auth": { "oidc_required": true, "password_login": false },
      "runtime": { "deepagents": true, "biwork_cli": false, "disabled": true, "remote_agent_direct": false },
      "desktop": {
        "gateway_required_for_local_capabilities": true,
        "shell": true,
        "office_preview": true,
        "preview_history": true,
        "local_remote_control": false,
        "cdp_remote_control": false
      },
      "enterprise": { "assistants": true, "providers": true, "conversations": true, "cron": true }
    },
    "catalog": {
      "assistants": [],
      "providers": [],
      "skills": [],
      "mcp_servers": [],
      "managed_agents": []
    },
    "route_ownership": {}
  }
}
```

### 6.9 Conversations

#### `POST /api/conversations`

Owner: `RUST`

Request minimum:

```json
{
  "type": "acp",
  "name": "New conversation",
  "assistant": {
    "id": "assistant_general"
  },
  "extra": {
    "workspace": "/workspace/demo",
    "preset_enabled_skills": ["skill_summary"],
    "selected_mcp_server_ids": ["mcp_docs"]
  }
}
```

Response:

- 返回值必须兼容 `TChatConversation`
- `created_at` / `modified_at` 必须是 number
- `runtime` 字段可选，但若存在必须兼容 `TConversationRuntimeSummary`
- 当 `assistant.id` 对应 enterprise Agent 时，Rust 必须在创建时解析并钉住最新 published AgentVersion；后续 message run 使用该版本编译 model/tool/skill/MCP/SQL bindings，不能只保存 `agent_id` 后以 `agent_version_id=null` 运行。没有 published version 时才允许保留 `null` fallback。

Example:

```json
{
  "success": true,
  "data": {
    "id": "conv_001",
    "name": "New conversation",
    "created_at": 1751900100000,
    "modified_at": 1751900100000,
    "type": "acp",
    "status": "finished",
    "runtime": {
      "state": "idle",
      "can_send_message": true,
      "has_task": false,
      "is_processing": false,
      "pending_confirmations": 0,
      "turn_id": null
    },
    "assistant": {
      "id": "assistant_general",
      "source": "builtin",
      "name": "General Assistant",
      "avatar": "",
      "backend": "deepagents"
    },
    "extra": {
      "workspace": "/workspace/demo",
      "backend": "deepagents",
      "skills": ["skill_summary"],
      "mcp_server_ids": ["mcp_docs"]
    }
  }
}
```

#### `GET /api/conversations/{id}`

Owner: `RUST`

Contract:

- 返回兼容 `TChatConversation`
- 404 时必须返回 `NOT_FOUND`

#### `POST /api/conversations/{id}/messages`

Owner: `RUST`

Request:

```json
{
  "content": "Please summarize the attached files",
  "files": [],
  "loading_id": "tmp-msg-123",
  "inject_skills": ["skill_summary"]
}
```

Response:

```json
{
  "success": true,
  "data": {
    "msg_id": "msg_user_001",
    "turn_id": "turn_001",
    "runtime": {
      "state": "running",
      "can_send_message": false,
      "has_task": true,
      "task_status": "running",
      "is_processing": true,
      "pending_confirmations": 0,
      "turn_id": "turn_001"
    }
  }
}
```

#### `GET /api/conversations/{id}/messages`

Owner: `RUST`

Contract:

- 返回 cursor page 对象，而不是裸数组。
- `items` 中每项兼容 `IResponseMessage` 经 `transformMessage()` 可消费的形状。
- `oldest_cursor/newest_cursor` 使用后端事件序列号字符串；没有对应消息时为 `null`。
- `has_more_before/has_more_after` 驱动 BiWork 历史分页。
- assistant event 没有显式 `message_id` 时，历史项必须使用 `assistant.{run_id}`，与实时 `message.stream` 保持同一合并身份。
- 同一 run 已存在 `message.completed` 时，不得再把其前置 `message.delta` 汇总成第二条历史消息；completed 内容是权威最终值。

Response shape:

```json
{
  "success": true,
  "data": {
    "items": [],
    "oldest_cursor": "100",
    "newest_cursor": "120",
    "has_more_before": true,
    "has_more_after": false
  }
}
```

Example item:

```json
{
  "type": "content",
  "data": {
    "content": "Hello"
  },
  "msg_id": "msg_assistant_001",
  "turn_id": "turn_001",
  "conversation_id": "conv_001",
  "created_at": 1751900123456,
  "position": "left",
  "status": "finish"
}
```

#### `GET /api/conversations/{id}/workspace`

Owner: `RUST`

Query:

- `path`: BiWork workspace path or `.` for the virtual root.
- `search`: optional full-text/path search keyword.

Contract:

- 返回数组。
- 普通目录浏览至少返回 `name` 和 `type`，其中 `type` 为 `file` 或 `directory`。
- 搜索响应必须额外返回 `full_path` 和 `relative_path`，否则 BiWork 会丢失嵌套文件路径。

Search response item:

```json
{
  "name": "report.md",
  "type": "file",
  "full_path": "/workspace/docs/nested/report.md",
  "relative_path": "docs/nested/report.md"
}
```

#### `POST /api/conversations/{id}/runtime/ensure`

Owner: `RUST`

Response:

```json
{
  "success": true,
  "data": {
    "recovered": false,
    "config_options": [],
    "runtime": {
      "state": "idle",
      "can_send_message": true,
      "has_task": false,
      "is_processing": false,
      "pending_confirmations": 0,
      "turn_id": null
    }
  }
}
```

#### `POST /api/conversations/{id}/active-lease`

Owner: `RUST`

Response:

```json
{
  "success": true,
  "data": {
    "leased_until_ms": 1751900400000
  }
}
```

#### `GET /api/conversations/{id}/slash-commands`

Owner: `RUST`

Contract:

- 返回数组
- 每项至少包含 `name`

#### `GET /api/conversations/{id}/confirmations`

Owner: `RUST`

Contract:

- 返回数组
- 每项兼容 `IConfirmation & { conversation_id: string }`
- Canonical option values are `proceed_once`, `proceed_always`, and `cancel`.
- Rust also accepts legacy aliases `allow_once`, `allow_always`, `deny`, `reject_once`, and `reject_always`.
- `approval.requested` 必须与 approval/tool_call/run 状态在同一事务写入 event/outbox 并投影为 `confirmation.add`；`proceed_once` resume 重放相同 `run + resource + args_hash` 时复用原已批准 tool call，不能再创建第二条 pending approval。
- 此 GET 返回当前 conversation 的权威 pending 快照。renderer 必须同时补入缺失卡片并移除快照中已不存在的 permission 卡片；轮询、focus/visibility 和实时事件触发若撞上正在进行的恢复请求，至少排队再执行一次，不能静默丢弃最新触发。

Example:

```json
{
  "success": true,
  "data": [
    {
      "conversation_id": "conv_001",
      "id": "approval_001",
      "title": "Permission required",
      "description": "Allow write_file to modify report.md?",
      "call_id": "tool_call_001",
      "action": "approve",
      "command_type": "write_file",
      "options": [
        { "label": "Allow once", "value": "proceed_once" },
        { "label": "Allow always", "value": "proceed_always" },
        { "label": "Cancel", "value": "cancel" }
      ]
    }
  ]
}
```

#### `POST /api/conversations/{id}/confirmations/{call_id}/confirm`

Owner: `RUST`

Request:

```json
{
  "msg_id": "approval_001",
  "data": { "value": "proceed_once" },
  "always_allow": false
}
```

Contract:

- 决策事务必须同时解析对应 open interrupt，并发布 confirmation update/remove 所需事实。
- conversation cancel/delete 必须按 conversation（以及请求中可选的 turn）关闭 pending approval、open interrupt 与 `waiting_approval` tool call；该规则同样适用于关联 run 已经 completed、但仍残留待审批事实的异常状态。

### 6.9.1 Team

#### `POST /api/teams` / `POST /api/teams/{team_id}/agents`

Owner: `RUST`

Contract:

- Request assistants must carry `assistant_id`; runtime backend is derived from the enterprise assistant catalog, not trusted from the renderer.
- Rust must enforce the same selectability contract as `GET /api/assistants`: only enabled `runtime.kind=deepagents` assistants are team-selectable in this compat path.
- Assistants with `team_selectable=false` must be rejected before insertion into `agent_team_members`, so a team cannot be created in a state that will inevitably fail at run dispatch.

#### `GET /api/teams/{team_id}/run-state`

Owner: `RUST`

Contract:

- Returns `{ "active_run": ITeamRunEvent | null }`.
- `active_run.slot_work[]` must include per-slot pending/active work for BiWork team status chips.
- Terminal team runs are omitted from `active_run`; realtime terminal `team.run*` events clear the local view.

### 6.10 Workbench / Files

All `/api/fs/*` and `/api/preview-history/*` facade/local file routes must validate the current Rust bearer session before reading request bodies, browsing local directories, writing uploads, or touching snapshot/preview-history state. `FACADE` only describes where the operation is implemented; it is not an unauthenticated local file capability.

#### `POST /api/fs/dir`

Owner: `FACADE`

Request:

```json
{
  "dir": "/workspace/src",
  "root": "/workspace"
}
```

Response:

- 返回 `IDirOrFile[]`

#### `POST /api/fs/read`

Owner: `FACADE`

Request:

```json
{
  "path": "/workspace/src/main.rs",
  "workspace": "/workspace"
}
```

Response:

- 返回字符串或 `null`

#### `POST /api/fs/metadata`

Owner: `FACADE`

Request:

```json
{
  "path": "/workspace/src/main.rs",
  "workspace": "/workspace"
}
```

Response:

```json
{
  "name": "main.rs",
  "path": "/workspace/src/main.rs",
  "size": 128,
  "type": "text/plain; charset=utf-8",
  "lastModified": 1720000000000,
  "isDirectory": false,
  "revision": 7,
  "etag": "sha256:..."
}
```

说明：

- `lastModified` 必须是 epoch milliseconds number。
- Rust-backed 文件必须返回 `revision/etag`，Preview 保存时用 `revision` 作为 `expected_revision`。
- 文件不存在时可返回 `type: "missing"`、`size: 0`、`lastModified: 0`，并省略 `revision/etag`。

#### `GET /api/v1/tool-result-artifacts/read`

Owner: `RUST`

Purpose:

- BiWork 展示大型 tool result artifact 的分页 preview 时使用该 enterprise 数据面。
- Renderer 侧通过 `FileService.fetchToolResultArtifactRead` 调用，普通 `httpRequest` 可解析 JSON envelope。
- `MessageToolGroupSummary` 的 `Preview` 按钮使用 `workbench.bootstrap.auth.tenant_id` 和 `views[].data_ref|artifact_ref.object_reference_id` 请求第一页，默认 `offset=0&limit=500`。

Query:

- `tenant_id`: required enterprise tenant id.
- `object_reference_id`: required artifact object reference id.
- `offset` / `limit`: optional row or character pagination for JSON array/JSONL/text artifacts.
- `offset_bytes` / `limit_bytes`: optional byte range query for text artifacts. 与 `offset/limit` 语义互斥。

Response:

- JSON envelope unwrapped by `httpRequest`, content shape is one of:
- `content.kind="json_rows"` with `rows`, `offset`, `limit`, `total_rows`.
- `content.kind="json_value"` with `value`.
- `content.kind="text"` with `text`, `offset`, `limit`, `total_chars`, `truncated`.
- `content.kind="text_byte_range"` with `text`, `offset_bytes`, `limit_bytes`, `total_bytes`, `truncated`.
- `content.kind="binary_metadata"` with `content_type`, `size_bytes`.

#### `GET /api/v1/tool-result-artifacts/stream`

Owner: `RUST`

Purpose:

- BiWork 需要展示或下载大型 tool result artifact 原始内容时使用该 enterprise 数据面。
- 该端点不返回 JSON envelope，响应体是 artifact raw bytes。
- Renderer 侧必须通过 `httpRawRequest` 或 `FileService.fetchToolResultArtifactStream` 调用，不能走会丢弃非 JSON response 的普通 `httpRequest`。
- `MessageToolGroupSummary` 可从 `acp_tool_call.content.update.rawOutput.views[].data_ref|artifact_ref` 读取 `object_reference_id`，用 `workbench.bootstrap.auth.tenant_id` 作为当前租户调用该端点并触发浏览器下载；同一 `views[]` 会在 renderer 侧做静态安全 preview，inline-data chart/map 可懒加载 `vega-embed` / `maplibre-gl` runtime 渲染并保留轻量 SVG/text fallback，带外联 `data.url` 的 chart spec 不进入 runtime，也可通过 read 端点按需加载分页内容，不执行任意 HTML/JS。

Query:

- `tenant_id`: required enterprise tenant id.
- `object_reference_id`: required artifact object reference id.
- `offset_bytes` / `limit_bytes`: optional byte range query. 与 HTTP `Range` header 互斥。

Headers:

- 可选 `Range: bytes=start-end`，仅支持单段 byte range。
- 同时传 `Range` 和 `offset_bytes/limit_bytes` 必须返回 validation error。

Response headers:

- `200 OK` for full content.
- `206 Partial Content` with `Content-Range` for range reads.
- `416 Range Not Satisfiable` with `Content-Range: bytes */<total>` for out-of-range reads.
- Always include `Accept-Ranges: bytes`, `Content-Type`, `Content-Length`, `x-content-sha256`, `x-object-reference-id`, and `x-file-revision` when available.

#### `POST /api/fs/image-base64`

Owner: `FACADE`

Request:

```json
{
  "path": "/workspace/assets/logo.png",
  "workspace": "/workspace"
}
```

Response:

- 返回完整 data URL 字符串，例如 `data:image/png;base64,iVBORw0KGgo=`
- 非图片或不存在返回 `null`

#### `POST /api/fs/write`

Owner: `FACADE`

Request minimum:

```json
{
  "path": "/workspace/src/main.rs",
  "workspace": "/workspace",
  "data": "fn main() {}",
  "expected_revision": "rev_123"
}
```

Error:

- revision 冲突必须返回 `409` + `WORKSPACE_REVISION_CONFLICT`
- `expected_revision` 接受 number 或 `rev_<number>` 字符串；Rust-backed Preview 保存必须传该字段，避免静默覆盖。

### 6.11 Cron

#### `GET /api/cron/jobs`

Owner: `RUST`

Contract:

- 返回 `ICronJob[]`

#### `POST /api/cron/jobs`

Owner: `RUST`

Request minimum:

```json
{
  "name": "Daily summary",
  "conversation_id": "conv_001",
  "created_by": "user",
  "schedule": {
    "kind": "cron",
    "expr": "0 9 * * *",
    "tz": "Asia/Shanghai",
    "description": "Every day at 09:00"
  },
  "message": "Summarize yesterday's changes",
  "execution_mode": "new_conversation",
  "agent_config": {
    "name": "General Assistant",
    "assistant_id": "assistant_general",
    "workspace": "/workspace/demo"
  }
}
```

Response:

- 返回 `ICronJob`

#### `PUT /api/cron/jobs/{job_id}`

Owner: `RUST`

Request is the flat BiWork scheduled-job update contract; clients must not send the wrapper `{ job_id, updates }`.

```json
{
  "name": "Daily summary",
  "enabled": true,
  "schedule": {
    "kind": "cron",
    "expr": "0 9 * * *",
    "tz": "Asia/Shanghai",
    "description": "Every day at 09:00"
  },
  "message": "Summarize yesterday's changes",
  "execution_mode": "existing",
  "agent_config": {
    "name": "General Assistant",
    "assistant_id": "assistant_general"
  },
  "conversation_title": "Workspace",
  "max_retries": 3
}
```

Notes:

- `message` is optional; clients may update only `execution_mode` without `target.payload`.
- Unknown or omitted fields must preserve existing job facts.

#### `POST /api/cron/jobs/{job_id}/run`

Owner: `RUST`

Request:

- Body is empty. Clients must not send `{ "job_id": "..." }`; the job id is path-only.

Response:

```json
{
  "success": true,
  "data": {
    "conversation_id": "conv_derived_001",
    "run_id": "run_001"
  }
}
```

Failure contract:

- Dispatch failure must still update the job facts: `last_status="error"`, `last_error`, `retry_count + 1`.
- It must append `scheduled_job_runs(status="failed")` with a non-secret `{trigger,error}` summary.
- It must write a `cron.manual` audit record.
- If the job has an existing conversation target, it must emit `cron.job-executed` with `{status:"error", job_id, cron_job_id, error}` before returning the error response.

#### `GET /api/cron/jobs/{job_id}/skill`

Owner: `RUST`

Contract:

- Returns `{ "has_skill": boolean }`.

#### `POST /api/cron/jobs/{job_id}/skill`

Owner: `RUST`

Contract:

- Request body must include non-empty `content`.
- Rust stores the skill content on the tenant/user-owned scheduled job and marks pending `skill_suggest` artifacts for that job as `saved`.
- If the job does not exist or is not owned by the current actor, return `404` instead of a silent success.

#### `DELETE /api/cron/jobs/{job_id}/skill`

Owner: `RUST`

Contract:

- Clears the saved skill content for the tenant/user-owned scheduled job.
- If the job does not exist or is not owned by the current actor, return `404` instead of a silent success.

### 6.12 Channel

#### `GET /api/channel/plugins`

Owner: `AGGREGATE`

Contract:

- 返回 `IChannelPluginStatus[]`
- 每项必须反映当前本地 connector 运行状态
- 如存在 enterprise governance 信息，应已经折叠进 `enabled/connected/status/extensionMeta`

#### `POST /api/channel/plugins/test`

Owner: `LOCAL`

Desktop gateway must validate the current Rust bearer session before reading the request body or invoking the local connector dry-run. Local ownership only means the connector runtime side effect is implemented by the desktop gateway.

Request minimum:

```json
{
  "plugin_id": "telegram",
  "token": "123456:ABCDEF"
}
```

Response:

```json
{
  "success": true,
  "data": {
    "success": true,
    "bot_username": "my_bot"
  }
}
```

#### `POST /api/channel/ingress/messages`

Owner: `RUST`

本地 connector 收到外部消息后通过 desktop gateway 调用该入口。Rust 必须验证 connector 已启用且外部用户已经在 `channel_authorized_users` 中处于 `active`，然后复用或创建 `channel_sessions`，必要时创建 channel conversation，最后通过 enterprise run gateway 创建 run。

要求：未启用 connector、未授权用户、已 revoke 用户都必须 fail closed，不创建 session/conversation/run。

Request minimum:

```json
{
  "platform_type": "telegram",
  "platform_user_id": "tg_10001",
  "chat_id": "chat_abc",
  "message_id": "msg_001",
  "content": "hello"
}
```

Response:

```json
{
  "success": true,
  "data": {
    "session_id": "4fb6f74c-47dd-49d1-916d-442d3bd0921e",
    "conversation_id": "7202ed5d-88fd-48b4-845e-c0f3c70ec039",
    "run_id": "f5f9d73c-3c68-4d59-84b0-7644e5b8d8aa",
    "created_conversation": true
  }
}
```

#### `GET /api/channel/settings/{platform}`

Owner: `RUST`

Response:

```json
{
  "success": true,
  "data": {
    "platform": "telegram",
    "assistant": {
      "assistant_id": "assistant_general"
    },
    "default_model": {
      "id": "provider_openai_1",
      "use_model": "gpt-4.1"
    }
  }
}
```

#### `GET /api/channel/pairings`

Owner: `RUST`

Contract:

- 返回 `IChannelPairingRequest[]`

#### `POST /api/channel/pairings/request`

Owner: `RUST`

本地 connector 收到外部用户绑定请求后通过 desktop gateway 调用该入口。Rust 写入 `channel_pairing_requests`，并通过 `/ws` 投影 `channel.pairing-requested`。

要求：对应 `platform_type` 的 channel connector 必须已启用，否则返回 `CONFLICT`，不创建 pairing。

Request minimum:

```json
{
  "platform_type": "telegram",
  "platform_user_id": "tg_10001",
  "display_name": "Alice",
  "ttl_seconds": 600
}
```

Response:

```json
{
  "success": true,
  "data": {
    "code": "PAIR1234",
    "platform_type": "telegram",
    "platform_user_id": "tg_10001",
    "display_name": "Alice",
    "requested_at": 1783531200000,
    "expires_at": 1783531800000
  }
}
```

#### `GET /api/channel/users`

Owner: `RUST`

Contract:

- 返回 `IChannelUser[]`

#### `GET /api/channel/sessions`

Owner: `RUST`

Contract:

- 返回 `IChannelSession[]`

### 6.13 Extensions / Hub

#### `GET /api/extensions`

Owner: `AGGREGATE`

Contract:

- 返回 `IExtensionInfo[]`
- 只返回当前设备已发现且通过 enterprise governance 过滤后的可见扩展

#### `GET /api/extensions/settings-tabs`

Owner: `AGGREGATE`

Contract:

- 返回 `IExtensionSettingsTab[]`
- `url` 必须是 gateway 白名单下的静态资源路径

#### `GET /api/extensions/channel-plugins`

Owner: `AGGREGATE`

Contract:

- 返回通过 Rust enterprise governance 过滤后的 extension channel plugin contribution manifests
- Desktop gateway 必须用该结果作为 `/api/channel/plugins` 本地 extension plugin 卡片的 allow-list，不能把 Rust 过滤掉的本地 contribution 再合并回页面

#### `GET /api/extensions/agent-activity`

Owner: `AGGREGATE`

Contract:

- Rust returns tenant-scoped run activity counters from enterprise conversation/run facts.
- Desktop gateway may append local extension runtime agent activity, but must keep Rust counters as the authoritative enterprise totals.
- Response shape follows BiWork `IExtensionAgentActivitySnapshot`.

Response:

```json
{
  "success": true,
  "data": {
    "generatedAt": 1783590400000,
    "totalConversations": 3,
    "runningConversations": 1,
    "agents": []
  }
}
```

#### `POST /api/extensions/sync`

Owner: `AGGREGATE`

Contract:

- Desktop gateway owns the renderer-facing route and must build the sync payload from local extension manifests under its whitelisted extension roots.
- Renderer/client request bodies are consumed for connection hygiene but must not be trusted as extension manifest input by the desktop gateway.
- Rust owns persistence, contribution governance projection, and `extension.sync` audit after receiving the gateway-built payload.
- If no local extension manifests are discovered, the gateway may return a zero-sync success response without writing a Rust audit entry.

Response:

```json
{
  "success": true,
  "data": {
    "synced": 1,
    "contributions": 3
  }
}
```

#### `POST /api/extensions/enable` / `POST /api/extensions/disable`

Owner: `AGGREGATE`

Contract:

- Desktop gateway must sync the current local extension manifest to Rust before submitting the toggle, so Rust can identify the governed package.
- Rust must accept the toggle and write `extension.enable` / `extension.disable` audit before the gateway commits local device state.
- If Rust rejects the toggle or audit fails, the gateway must not persist the local enable/disable state.
- After local state is committed, the gateway must sync the new contribution enabled state back to Rust.

#### `GET /api/hub/extensions`

Owner: `AGGREGATE`

Contract:

- 返回 `IHubAgentItem[]`
- `status` 必须反映本地安装状态
- enterprise governance 状态由 Rust projection 合并，不由本地 hub installer 决定

#### `POST /api/hub/install`

Owner: `LOCAL`

Local hub mutation routes (`install`, `uninstall`, `retry-install`, `update`, and `check-updates`) still must validate the current Rust bearer session before reading the request body or touching local hub state. Local ownership only means the installer side effect is implemented by the desktop gateway; it does not allow unauthenticated extension state changes.

Installer invariants:

- catalog item 必须来自受治理的 `GET /api/hub/extensions`，renderer 不能提交任意下载 URL。
- tarball 支持 HTTP(S)；离线/内置 catalog 可使用受 256 MiB 上限约束的 base64 data URL。
- `dist.integrity` 必须是 `sha512-<base64>` SRI，校验失败不得解压或覆盖现有安装。
- extension name 只允许安全的单路径段；tar 解压禁止 preserve paths，并且解压后 `aion-extension.json.name` 必须与请求 name 一致。
- install/update 使用 staging directory 和 backup rename 原子替换；失败时保留旧安装并记录 `install_failed + error`。
- 每次 install/retry/update/uninstall 后，desktop gateway 必须调用 `/api/extensions/sync`，再把 `governanceSync` 摘要返回 renderer。

Request:

```json
{
  "name": "ext-claude-code"
}
```

Response:

```json
{
  "success": true,
  "data": {
    "name": "ext-claude-code",
    "status": "installed",
    "governanceSync": {
      "synced": 1,
      "contributions": 2
    }
  }
}
```

`POST /api/hub/uninstall` 使用同一 request shape；成功返回 `status: "not_installed"`，并在 governance sync 后禁用该 package 的 contributions。

### 6.14 Shell / Office Preview

#### `POST /api/shell/open-file`

Owner: `LOCAL`

Request:

```json
{
  "file_path": "/workspace/report.pdf"
}
```

Other local shell actions use the same desktop-only plane and must not fall through to Rust:

| Route | Request body |
| --- | --- |
| `POST /api/shell/show-item-in-folder` | `{ "file_path": "/workspace/report.pdf" }` |
| `POST /api/shell/open-external` | `{ "url": "https://example.com/docs" }` |
| `POST /api/shell/check-tool-installed` | `{ "tool": "officecli" }` |
| `POST /api/shell/open-folder-with` | `{ "folder_path": "/workspace", "tool": "vscode" }` |

#### `POST /api/document/convert`

Owner: `LOCAL`

Request:

```json
{
  "file_path": "/workspace/report.docx",
  "to": "markdown",
  "workspace": "/workspace"
}
```

#### `POST /api/ppt-preview/start`

Owner: `LOCAL`

Request:

```json
{
  "file_path": "/workspace/report.pptx",
  "workspace": "/workspace"
}
```

Response:

```json
{
  "success": true,
  "data": {
    "url": "/api/ppt-proxy/38125"
  }
}
```

`/api/word-preview/start` and `/api/excel-preview/start` use the same request shape. Their corresponding `*/stop` routes take only `{ "file_path": "..." }`. The gateway emits `ppt-preview.status`, `word-preview.status`, and `excel-preview.status` on the global BiWork event bus.

#### `GET /api/ppt-proxy/{port}` / `GET /api/office-watch-proxy/{port}`

Owner: `LOCAL`

These routes are iframe-only Office watch proxy routes. The desktop gateway must proxy them only to `127.0.0.1:{port}` and must reject invalid ports or non-GET/HEAD methods. They are not enterprise resource routes and must not fall through to Rust.

## 7. WebSocket Contract

### 7.1 Client -> Gateway Frames

#### Auth

客户端必须在共享 token broker 拿到 access token 后再建立 `/ws` 或发送任何队列帧；连接建立后的第一帧必须是 `auth`。

```json
{
  "op": "auth",
  "access_token": "eyJ..."
}
```

#### Subscribe

```json
{
  "op": "subscribe",
  "scope": "conversation",
  "id": "conv_001"
}
```

Allowed scopes:

```text
conversation
team
cron
channel
extensions
hub
```

### 7.2 Server -> Client Envelope

所有事件统一形状：

```json
{
  "name": "message.stream",
  "data": {}
}
```

### 7.3 `message.stream`

Payload 最低合同等同于 `IResponseMessage`：

```json
{
  "type": "content",
  "data": {
    "content": "stream chunk"
  },
  "msg_id": "msg_assistant_001",
  "turn_id": "turn_001",
  "conversation_id": "conv_001",
  "created_at": 1751900123456,
  "position": "left",
  "status": "pending",
  "replace": false,
  "hidden": false
}
```

Allowed `type` minimum set:

```text
content
text
user_content
tips
error
tool_call
tool_group
agent_status
permission
acp_permission
acp_tool_call
plan
thinking
available_commands
```

同一 run 的 `message.delta` 与 `message.completed` 必须使用同一 `msg_id`。没有上游 `message_id` 时统一使用 `assistant.{run_id}`；完成事件发送完整内容并设置 `replace: true`，BiWork 据此替换已累计的 delta，而不是追加第二条回复。

Python runtime 在平台事件边界必须移除 `<think>`、`<thinking>`、`<analysis>`、`<reasoning>` 块；这些内部推理不得进入 `message.delta`、`message.completed.content` 或公开 result message 字段。过滤器必须支持 tag 跨 chunk，纯 reasoning chunk 不发送空白消息。

平台 wrapper 的治理 `tool_call_id` 仍用于 Rust `tool_calls` 审批/审计事实；当 DeepAgents 已为同一 file tool 产生 stream call id 时，terminal payload 额外携带 `ui_tool_call_id`。BiWork WS 投影优先使用 `ui_tool_call_id` 生成 `msg_id` 和 `update.tool_call_id`，把 started/delta 与治理后的 completed/failed 合并为同一工具卡，不能显示两个步骤。

### 7.4 `turn.completed`

Payload 最低合同等同于 `IConversationTurnCompletedEvent`：

```json
{
  "session_id": "conv_001",
  "turn_id": "turn_001",
  "status": "finished",
  "state": "ai_waiting_input",
  "detail": "",
  "can_send_message": true,
  "runtime": {
    "state": "idle",
    "can_send_message": true,
    "has_task": false,
    "task_status": "finished",
    "is_processing": false,
    "pending_confirmations": 0,
    "turn_id": null
  },
  "workspace": "/workspace/demo",
  "model": {
    "platform": "openai-compatible",
    "name": "OpenAI Compatible",
    "use_model": "gpt-4.1"
  },
  "last_message": {
    "id": "msg_assistant_001",
    "type": "content",
    "content": { "content": "done" },
    "status": "finish",
    "created_at": 1751900123999
  }
}
```

### 7.5 `confirmation.add` / `confirmation.update` / `confirmation.remove`

实时事件用于低延迟更新，`GET /api/conversations/{id}/confirmations` 仍是恢复与对账的权威来源；客户端必须能从漏失 add、update 或 remove 事件后恢复到服务端快照。

`confirmation.add` 和 `confirmation.update` 的 payload 至少兼容：

```json
{
  "conversation_id": "conv_001",
  "id": "approval_001",
  "description": "Allow write_file to modify report.md?",
  "call_id": "tool_call_001",
  "options": [
    { "label": "Allow once", "value": "proceed_once" },
    { "label": "Allow always", "value": "proceed_always" },
    { "label": "Cancel", "value": "cancel" }
  ]
}
```

`confirmation.remove`:

```json
{
  "conversation_id": "conv_001",
  "id": "approval_001"
}
```

### 7.6 `conversation.listChanged`

Payload:

```json
{
  "conversation_id": "conv_001",
  "action": "updated",
  "source": "run_completed"
}
```

### 7.7 `conversation.artifact`

Payload must match `IConversationArtifact`:

- `cron_trigger`
- `skill_suggest`

Example:

```json
{
  "id": "artifact_001",
  "conversation_id": "conv_001",
  "kind": "cron_trigger",
  "status": "active",
  "payload": {
    "cron_job_id": "cron_001",
    "cron_job_name": "Daily summary",
    "triggered_at": 1751900200000
  },
  "created_at": 1751900200000,
  "updated_at": 1751900200000
}
```

### 7.8 `cron.job-*`

Events:

```text
cron.job-created
cron.job-updated
cron.job-removed
cron.job-executed
```

Payloads:

- `cron.job-created` / `cron.job-updated`: `ICronJob`
- `cron.job-removed`: `{ "job_id": "cron_001" }`
- `cron.job-executed`:

```json
{
  "job_id": "cron_001",
  "cron_job_id": "cron_001",
  "cron_job_name": "Daily summary",
  "status": "ok",
  "error": null,
  "conversation_id": "conv_001",
  "run_id": "run_001",
  "triggered_at": 1783569600000
}
```

Failure payloads must keep `job_id`, `cron_job_id`, `status:"error"` and `error`.

### 7.9 `channel.*`

Required events:

```text
channel.pairing-requested
channel.plugin-status-changed
channel.user-authorized
```

Payloads:

- `channel.pairing-requested`: `IChannelPairingRequest`
- `channel.user-authorized`: `IChannelUser`
- `channel.plugin-status-changed`:

```json
{
  "plugin_id": "telegram",
  "status": {
    "id": "telegram",
    "type": "telegram",
    "name": "Telegram",
    "enabled": true,
    "connected": true,
    "activeUsers": 5
  }
}
```

### 7.10 `extensions.state-changed` / `hub.state-changed`

`extensions.state-changed`:

```json
{
  "name": "ext-claude-code",
  "enabled": true,
  "reason": "user_toggle"
}
```

`hub.state-changed`:

```json
{
  "name": "ext-claude-code",
  "status": "installed",
  "error": null
}
```

### 7.11 `fileStream.contentUpdate`

Payload:

```json
{
  "file_path": "/workspace/report.md",
  "content": "# updated",
  "operation": "write"
}
```

用途：

- PreviewContext 流式刷新
- 不替代 enterprise 文件事实源

## 8. Compatibility Rules

### 8.1 Compat `/api/*` != Enterprise `/api/v1/*`

允许差异：

1. 时间字段格式
2. DTO 命名
3. envelope 包装
4. 聚合字段和前端便利字段

不允许差异：

1. 权限事实源
2. tenant 隔离
3. 审批/审计事实
4. run / tool / file / channel enterprise 资源治理

### 8.2 Fail Closed

以下情况必须失败，不允许 silent fallback：

- 没有 access token 调 enterprise 路由
- token 已 revoke
- 当前用户没有 tenant membership
- channel connector 未获准但试图创建 run
- extension contribution 未通过 governance 却试图注入 assistant/skill/mcp
- local CLI runtime 未启用却试图执行

### 8.3 WebUI 降级

以下能力允许降级，但必须可见地降级：

- shell
- Office preview
- preview history
- hub install
- local channel connector dry-run

## 9. Contract Tests

建议固定以下测试资产：

```text
docs/contracts/biwork/
  auth-user.json
  settings-client.json
  assistants-list.json
  assistant-detail.json
  conversation-create.json
  conversation-send-message.json
  conversation-history.json
  turn-completed.json
  confirmation-add.json
  cron-job.json
  channel-plugin-status.json
  extension-info.json
```

建议测试类型：

1. Rust compat contract tests：
   - 固定 JSON snapshot
2. BiWork unit tests：
   - 固定 mock response
3. `agent-browser` / Playwright screenshot tests：
   - 用这些 fixture 驱动页面

如果实现结果和本文档不一致，应先改文档并说明破坏点，再改代码；不要让代码和合同长期漂移。
