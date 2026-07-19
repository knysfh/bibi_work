# BiWork 企业级智能体平台改造执行方案

更新日期：2026-07-14

## 1. 目标与边界

目标是把当前 `bibi_work_backend` Rust 控制面、`bibi_work_agent` Python agent runtime 和 `BiWork` Electron/React 桌面端收敛为一套企业级智能体运行平台。

本轮目标：

1. Rust 后端作为企业资源的唯一可信控制面，负责登录态校验、租户隔离、资源级授权、审批、审计、Agent/Skill/Tool/MCP/LLM 管理、Run Gateway、定时任务、Channel 绑定、文件和记忆治理。
2. Python agent runtime 作为受控运行面，只执行 Rust 下发的 run snapshot；所有工具、文件、本地执行、MCP、SQL 和第三方副作用都回调 Rust 二次鉴权。
3. BiWork 保留现有布局、页面风格、会话、文件预览、编辑、工作区、Skill/MCP 设置、Agent CLI 管理、Cron、Channel、Extensions、Hub 等体验，优先替换认证、数据源和运行边界，不重建 UI。
4. `shell / Office preview / Hub 安装器 / Extension 本地运行时 / Channel connector` 这类明显属于桌面本地能力的动作，不强行压进 Rust 控制面；它们保留在 BiWork 桌面本地能力平面，由桌面 gateway 暴露给 renderer。
5. 旧 `bibi_work_frontend` 已删除，本方案不从 git 历史恢复，也不继续以旧 Tauri 前端作为实现基线。
6. 远程操控、CDP、remote-agent 直连握手、真实本地远程操控等能力本轮不适配，可以保留 stub 或隐藏入口。

需要明确的一点：BiWork 当前是 Electron + React 项目，不是 Tauri。本方案按 Electron 主进程、preload、renderer、WebUI host 的实际源码处理。旧 Tauri local executor 思路只作为后续本地执行协议参考，不在本轮强行迁回。

另外，本方案把 BiWork 桌面端视为：

```text
renderer-facing desktop gateway
  + enterprise proxy
  + local capability plane
```

也就是说，renderer 看到的仍然是一组统一的 `/api/*` 和 `/ws`，但它们不再等价于“全部由 Rust 提供”。

## 2. 已确认的源码事实

### 2.1 BiWork 现状

关键源码：

- `bibi_work_frontend/packages/desktop/src/renderer/main.tsx`：启动时会先初始化 `configService`，再挂 `AuthProvider`、`ThemeProvider`、`PreviewProvider`、`FeedbackProvider`。
- `bibi_work_frontend/packages/desktop/src/renderer/components/layout/Router.tsx`：现有路由包括 `/guid`、`/conversation/:id`、`/team/:id`、`/scheduled`、`/settings/*`、`/assistants`、`/login`。
- `bibi_work_frontend/packages/desktop/src/common/adapter/httpBridge.ts`：统一 HTTP/WS bridge，当前发往 `/api/*` 和 `/ws`，没有携带 FerrisKey Bearer token；桌面模式默认直连 `window.__backendPort`。
- `bibi_work_frontend/packages/desktop/src/common/config/configService.ts`：直接 `fetch('/api/settings/client')`，不走 `httpBridge`，目前也没有 token broker。
- `bibi_work_frontend/packages/desktop/src/common/config/configMigration.ts`、`packages/desktop/src/process/utils/*`：主进程和迁移逻辑也会直接调用 `/api/settings/client`、`/api/providers`、`/api/channel/*` 等。
- `bibi_work_frontend/packages/desktop/src/common/adapter/ipcBridge.ts`：BiWork 真实依赖的接口面不只会话和技能，还包括 `/api/cron/*`、`/api/channel/*`、`/api/extensions/*`、`/api/hub/*`、`/api/shell/*`、`/api/ppt-preview/*`、`/api/word-preview/*`、`/api/excel-preview/*`。
- `bibi_work_frontend/packages/desktop/src/renderer/hooks/context/AuthContext.tsx`：桌面模式当前直接置为 authenticated；WebUI 使用旧 `/login`、`/logout`、`/api/auth/user` cookie 语义。
- `bibi_work_frontend/packages/desktop/src/renderer/pages/conversation/Workspace/*` 和 `Preview/*`：文件树、预览、编辑、diff、preview-history 体验完整，适合保留。
- `bibi_work_frontend/packages/desktop/src/common/adapter/ipcBridge.ts` 中 `shell`、`pptPreview`、`wordPreview`、`excelPreview`：这些是明显桌面本地动作，不应被错误改造成 Rust enterprise API。
- `bibi_work_frontend/packages/desktop/src/common/adapter/ipcBridge.ts` 中 `cron`、`channel`、`extensions`、`hub`：这些能力已经被页面真实依赖，不能简单返回空列表。
- `bibi_work_frontend/packages/web-host/src/static-server.ts`：WebUI 当前会把 `/api/*`、`/login`、`/logout` 和 `/ws` 代理给后端。

### 2.2 Rust 后端现状

关键源码：

- `bibi_work_backend/src/startup.rs`：当前后端只暴露 `/api/v1/*` 和 `/internal/*`，受保护 API 通过 FerrisKey access token middleware。
- `bibi_work_backend/src/features/agent_platform/ferriskey_oidc.rs`：已实现 FerrisKey JWT 校验、JWKS、audience/azp、session/device/user 投影。
- `bibi_work_backend/src/features/agent_platform/mod.rs`：已有 `/api/v1/workbench/*`、`/api/v1/agent-teams/*`、Agent/Skill/Tool/MCP/LLM/Policy/Run/Approval/Memory/Workflow/File API。
- `bibi_work_backend/src/features/agent_platform/handlers/workbench_service.rs`：已有 BiWork workbench 方向的 bootstrap、workspace detail、conversation detail、file tree、file preview、file diff、artifact preview。
- `bibi_work_backend/src/features/agent_platform/handlers/agent_team_service.rs`：已有企业 agent team 表和多成员 run 创建。
- `bibi_work_backend/migrations/20260707000001_biwork_workbench.sql`：已有 `user_ui_preferences`、`workspace_pins`、`run_event_links`、`artifact_previews` 等 BiWork workbench 支撑表。
- `bibi_work_backend/migrations/20260707000002_agent_teams.sql`：已有 `agent_teams`、`agent_team_members`、`agent_team_runs`、`agent_team_run_members`。

主要缺口：

1. Rust 目前提供的是企业 API，不是 BiWork renderer 真实可直接消费的 compat 合同。
2. BiWork 不只是 `httpBridge.ts` 需要 token，`configService.ts`、`configMigration.ts`、主进程辅助逻辑也需要统一的 token broker。
3. Rust public API 现在大量要求 `tenant_id` 参数，BiWork 旧接口没有这个概念，需要 compat 层从 FerrisKey ctx 默认租户推导。
4. Rust 当前只有 conversation-scoped WS/SSE；BiWork 需要全局 `/ws` 事件总线，而且其中一部分事件本来就不是 Rust 负责产生的。
5. Rust 已有 enterprise catalog，但缺 BiWork `Assistant`、`ManagedAgent`、`Provider`、`MCP`、`Cron`、`Channel settings` 等响应形状适配。

### 2.3 Python agent runtime 现状

关键源码：

- `bibi_work_agent/api/schemas.py`：已有 run dispatch/resume/cancel/tool authorize/event ingest schema。
- `bibi_work_agent/runtime/agent_factory.py`：已按 `run_config_snapshot` 构造 deepagents、PlatformCompositeBackend、ToolWrapper、runtime credential。
- `bibi_work_agent/backends/platform_composite_backend.py`：已支持 `/workspace/`、`/local/main/`、`/scratch/`、`/artifacts/` 等虚拟路径。
- `bibi_work_agent/tools/wrapper.py`：工具前置 Rust 鉴权，支持 allow/review/deny、HITL interrupt、结果脱敏、`ToolResultView`。
- `bibi_work_agent/runtime/event_normalizer.py`：已把 deepagents 事件归一化为平台事件。

主要缺口：

1. Python 事件是平台事实事件，不直接输出 BiWork `IResponseMessage`；投影必须留在 Rust compat 层或桌面 gateway 事件桥。
2. `runtime.kind` 边界已收敛为 `deepagents -> Python runtime`、`biwork_cli -> desktop local runtime`、`disabled -> catalog-only`；Rust dispatch 和 Python execute/resume 都会拒绝非 `deepagents`，避免桌面 CLI 误进 Python runtime。
3. `/local/main/` 目前只是一套虚拟路径协议，真实桌面 local runtime 仍未接入。

### 2.4 本轮必须承认的边界事实

本轮不能再假设“所有 `/api/*` 都由 Rust 直接提供”。

更准确的事实是：

```text
BiWork renderer
  -> desktop gateway (/api/*, /ws)
       -> Rust enterprise compat API / enterprise WS
       -> local shell / office preview / extension runtime / hub manager / channel connector
```

如果不先把这个边界写清楚，`shell`、Office preview、Extensions、Hub、Channel 的能力就会在执行中被误删或误迁。

## 3. 总体策略

本轮采用“桌面 gateway 分流 + Rust enterprise compat/BFF + Python 受控运行面”的策略。

### 3.1 分层原则

1. Rust 后端仍然是企业资源的唯一可信控制面。任何会创建 run、访问租户资源、修改策略、执行工具、绑定 assistant/skill/tool/MCP/channel、写审计的动作，都必须最终进入 Rust。
2. BiWork desktop gateway 是 renderer-facing facade，不是权限事实源。它负责：
   - 给 renderer 暴露统一 `/api/*` 和 `/ws`
   - 把 enterprise 路由代理到 Rust compat API
   - 保留桌面本地能力路由和事件
   - 合并本地事件与 Rust enterprise 事件
3. Python 不感知 BiWork UI contract，只认 Rust 下发的 enterprise `run_config_snapshot`。
4. `shell / Office preview / Extension 安装管理 / Hub 安装器 / Channel connector 进程` 是桌面本地能力，不进入 Rust 资源授权边界；但它们如果要影响企业资源，仍必须经过 Rust。
5. 所有新增/改造必须默认 fail closed：无 token、无 tenant membership、无 policy binding、工具高风险未审批、未通过 extension/channel policy 的本地连接器，都不执行。

### 3.2 路由归属原则

桌面模式下，renderer 统一打到 desktop gateway：

```text
/api/auth/*
/api/me
/api/settings*
/api/assistants*
/api/agents*
/api/skills*
/api/mcp/*
/api/providers*
/api/conversations*
/api/teams*
/api/cron/*
    -> proxy to Rust enterprise compat

/api/shell/*
/api/ppt-preview/*
/api/word-preview/*
/api/excel-preview/*
/api/hub/install|uninstall|retry-install|check-updates|update
/api/channel/plugins/test
    -> desktop local capability handlers

/api/extensions/*
/api/hub/extensions
/api/channel/plugins
    -> desktop aggregate handlers
       + Rust governance projection

/ws
    -> desktop WS multiplexer
       + Rust enterprise event stream
       + local capability events
```

说明：

- `channel` 不是完全本地，也不是完全 Rust。建议分成：
  - enterprise facts：assistant/default model 绑定、pairing、authorized users、channel session、审计，放 Rust。
  - local runtime：具体 connector 进程、plugin dry-run/test、连接状态探活，放 desktop local capability plane。
- `extensions/hub` 也是类似：本地安装与加载在 BiWork；企业可见性、允许启用、贡献物治理、审计和对 run 的可用性判断在 Rust。

### 3.3 推荐模块边界

Rust 侧推荐新增：

```text
bibi_work_backend/src/features/biwork_compat/
  mod.rs
  auth.rs
  settings.rs
  assistants.rs
  agents.rs
  providers.rs
  conversations.rs
  messages.rs
  workspace.rs
  skills.rs
  mcp.rs
  teams.rs
  cron.rs
  channel.rs
  approvals.rs
  ws.rs
  event_projection.rs
  dto.rs
```

BiWork 桌面侧推荐新增：

```text
bibi_work_frontend/packages/desktop/src/process/gateway/
  authTokenBroker.ts
  routeOwnership.ts
  enterpriseProxy.ts
  wsBridge.ts
  localShellRoutes.ts
  officePreviewRoutes.ts
  extensionRoutes.ts
  hubRoutes.ts
  channelRoutes.ts
```

不要继续把所有兼容逻辑堆进一个 `support.ts` 或一个“大而全 backend 适配文件”。

## 4. 目标数据模型调整

当前已经有 enterprise 主表，允许破坏性更新，建议直接按下面方式收敛。

### 4.1 认证、用户、会话、设备

保留并强化：

```text
platform_users
platform_sessions
devices
user_tenant_memberships
ferriskey_role_projection
resource_relations
resource_policy_bindings
authz_decisions
audit_logs
user_ui_preferences
```

调整点：

1. `platform_sessions` 增加 `client_kind`：`desktop|web|runtime`。
2. `devices` 增加 `app_kind`：`biwork-desktop|biwork-web|local-executor|unknown`。
3. `platform_sessions` 增加 `revocation_reason`，供 revoke 后主动断开 WS。
4. `user_ui_preferences` 作为 BiWork `configService` 的服务端事实源；未登录时 BiWork 只使用内建默认值，不再依赖旧 cookie/session。

### 4.2 Assistant 与 Enterprise Agent 的映射

BiWork 的 `Assistant` 是用户可选的 UI 预设，Enterprise `AgentVersion` 是运行事实。

建议保留 `assistant_profiles`，但把绑定表改成归一化结构，不再用多个可空外键：

```text
assistant_profiles
- id uuid
- tenant_id uuid
- owner_user_id uuid null
- agent_id uuid not null
- default_agent_version_id uuid null
- source text: builtin|generated|user|extension
- name text
- description text
- avatar text null
- locale_overrides jsonb
- defaults jsonb
- enabled bool
- sort_order int
- team_selectable bool
- metadata jsonb
- created_at timestamptz
- updated_at timestamptz
- deleted_at timestamptz

assistant_profile_capability_bindings
- id uuid
- tenant_id uuid
- assistant_profile_id uuid
- capability_type text: skill|mcp_tool|tool
- capability_id uuid not null
- capability_version_id uuid null
- default_enabled bool
- disabled_by_default bool
- load_order int
- policy_snapshot jsonb
- created_at timestamptz
- updated_at timestamptz
```

运行时不直接引用 `assistant_profiles`，而是在创建 conversation/run 时解析为：

- `agent_id`
- `agent_version_id`
- capability selection
- capability policy snapshot

### 4.3 Provider 与 Enterprise LLM

BiWork `/api/providers/*` 映射到：

```text
llm_providers
llm_credentials
llm_model_profiles
```

调整点：

1. BiWork `provider.id` 对应 `llm_provider.id` 或 `llm_model_profile.id` 时必须明确，不再混用。
2. API key 不返回前端。BiWork 编辑页只展示 `has_secret_ref`、`secret_hint`、`rotation_status`。
3. `fetchProviderModels` 和 `modelProfile:test` 都由 Rust 调用 provider，不让 BiWork renderer 直接测密钥。
   - 已推进：BiWork 模型健康检查改走 `/api/providers/{provider_id}/test`；Rust compat 会按 provider/model 找 active `llm_model_profiles`，复用 enterprise `llm-model-profiles/{profile_id}/test`，由 Rust 解析 active credential 并校验 provider models response 中存在目标 `model_name`。

### 4.4 Workbench、Workspace 与 Project

保留：

```text
workspaces
local_mounts
workspace_pins
projects
project_mounts
file_revisions
file_locks
object_references
tool_result_artifacts
artifact_previews
```

调整点：

1. BiWork `workspace` 字符串只作为显示和路径参数，不作为权限边界。
2. 企业事实使用 `workspace_id` + `remote_project_id` + `local_mounts`。
3. `/api/fs/*` 只作为 BiWork compat 包装，内部调用 Rust enterprise file/workbench service。
4. 用户拖拽/粘贴本地文件，本轮只支持：
   - 上传到 RustFS-backed project
   - 或写入已经明确授权的 local mount

### 4.5 审批与审计

保留并强化：

```text
approvals
interrupts
tool_calls
audit_logs
approval_evidence
tool_call_evidence
audit_hash_chain_segments
```

调整点：

1. BiWork `confirmation.*` 只是 UI 投影，不是审批事实源。
2. 不要把 `biwork_confirmation_shape` 直接塞进 `approvals.request_payload`。compat 层应从 `approval + tool_call + obligations + run context` 动态投影 UI 卡片。
3. 如果后续确实需要缓存 UI 形状，新增独立的 `approval_presentations` 或 `ui_projection_cache` 表，不污染审批核心事实。
4. 审批 decision API 只接受当前 Rust actor，不接受前端伪造 `actor_user_id`。

### 4.6 消息投影

建议新增 message projection 表，避免每次都把 `run_events` 全量回放成 BiWork `TMessage`：

```text
conversation_messages
- id uuid
- tenant_id uuid
- conversation_id uuid
- run_id uuid null
- source_event_id uuid null
- source_seq bigint not null
- msg_id text not null
- turn_id text null
- type text not null
- position text
- status text
- content jsonb not null
- hidden bool not null default false
- projection_version text not null
- created_at timestamptz
- updated_at timestamptz
- unique (tenant_id, conversation_id, source_seq)
```

第一版也可以先不建表，由 `event_projection.rs` 从 `run_events` 动态投影；如果分页和恢复性能不够，再落表。

### 4.7 定时任务（Cron）

不要再把 Cron 视为前端附属功能。它本质上是企业级 scheduled run。

建议增加：

```text
scheduled_jobs
- id uuid
- tenant_id uuid
- source_conversation_id uuid
- target_mode text: existing|new_conversation
- target_conversation_id uuid null
- assistant_profile_id uuid not null
- agent_snapshot jsonb not null
- prompt_template text not null
- workspace_id uuid null
- model_profile_id uuid null
- schedule_kind text: at|every|cron
- schedule_expr text not null
- timezone text null
- enabled bool
- created_by_user_id uuid
- created_from text: user|agent
- next_run_at timestamptz null
- last_run_at timestamptz null
- last_status text null
- last_error text null
- run_count int
- retry_count int
- max_retries int
- created_at timestamptz
- updated_at timestamptz

scheduled_job_runs
- id uuid
- tenant_id uuid
- scheduled_job_id uuid
- run_id uuid null
- workflow_run_id uuid null
- status text
- triggered_at timestamptz
- completed_at timestamptz null
- summary jsonb

scheduled_job_artifacts
- id uuid
- tenant_id uuid
- scheduled_job_id uuid
- artifact_kind text: cron_trigger|skill_suggest
- object_reference_id uuid null
- payload jsonb
- created_at timestamptz
```

### 4.8 Extension / Hub / Channel 治理

Extensions、Hub、Channel 不能简单视为“桌面本地黑盒”。本轮建议把治理事实收敛到 Rust，把安装和运行保留在桌面端。

建议增加：

```text
extension_packages
- id uuid
- tenant_id uuid
- extension_name text
- source text: bundled|hub|local
- version text
- integrity text
- manifest jsonb
- risk_level text
- status text: discovered|approved|blocked|disabled
- created_at timestamptz
- updated_at timestamptz

device_extension_states
- id uuid
- tenant_id uuid
- device_id uuid
- extension_package_id uuid
- installed bool
- enabled bool
- install_status text
- last_error text null
- updated_at timestamptz

extension_contributions
- id uuid
- tenant_id uuid
- extension_package_id uuid
- contribution_type text:
  assistant|agent|skill|mcp_server|channel_plugin|webui|theme|settings_tab|acp_adapter
- contribution_key text
- manifest jsonb
- enabled bool
- created_at timestamptz
- updated_at timestamptz

channel_connectors
- id uuid
- tenant_id uuid
- connector_key text
- source_extension_package_id uuid null
- runtime_kind text: builtin|extension
- status text
- config_ref jsonb
- updated_at timestamptz

channel_platform_bindings
- id uuid
- tenant_id uuid
- platform_key text
- assistant_profile_id uuid null
- default_model_profile_id uuid null
- connector_id uuid null
- enabled bool
- created_at timestamptz
- updated_at timestamptz

channel_pairings
channel_authorized_users
channel_sessions
```

说明：

1. 本地扩展包的安装目录、解压路径、OfficeCLI 端口等设备瞬时状态，不必写进 Rust 企业表。
2. 但扩展 manifest、channel plugin 能力、是否允许启用、贡献物与 assistant/agent/skill/mcp/channel 的绑定和审计，必须进入 Rust。
3. `channel plugin` 可以来源于 builtin，也可以来源于 hub/extension。

### 4.9 桌面本地能力边界

本轮不为下面这些动作设计 Rust 企业事实表：

- `shell.openFile`
- `shell.showItemInFolder`
- `shell.openExternal`
- `shell.checkToolInstalled`
- `shell.openFolderWith`
- `pptPreview`
- `wordPreview`
- `excelPreview`

原因很简单：这些是桌面便利能力，不是企业资源事实。

但要注意：

1. 它们不能反向成为权限事实源。
2. 它们如果消费的是企业文件，文件本身仍然要先经过 Rust 文件服务获取或挂载授权。
3. 它们只在 desktop gateway 的本地路由上保留；纯 WebUI 模式下按 feature flag 隐藏或只读降级。

## 5. Renderer-facing API 与事件映射

本章是接口边界摘要；具体字段、envelope、WS payload、route ownership 归属以 [biwork-api-contract.md](./biwork-api-contract.md) 为准。

### 5.1 总入口约定

桌面模式：

```text
renderer -> desktop gateway
  HTTP: /api/*
  WS:   /ws
```

浏览器 WebUI 模式：

- 仍可走统一 `/api/*` 和 `/ws`
- 但若当前运行环境没有 desktop gateway，本地能力路由必须降级或隐藏

### 5.2 认证与 token broker

所有以下调用方都必须改成共享的 token broker，而不是各自直接 `fetch`：

- `httpBridge.ts`
- `configService.ts`
- `configMigration.ts`
- `packages/desktop/src/process/utils/*`
- future desktop gateway WS bridge

约束：

1. 统一由 `authTokenBroker.ts` 持有 access token。
2. renderer、main process、desktop gateway 共用同一 token 读取接口。
3. token 不放 `localStorage`。
4. refresh token 如需持久化，只放 Electron main 的安全存储或 OS keychain。
5. 未登录时：
   - login 页面使用内建默认主题/语言
   - `configService.initialize()` 不阻塞应用启动
   - enterprise `/api/settings/client` 在没有 token 时不应该被假装成功

### 5.3 认证与用户上下文

| Renderer-facing 入口 | 实际归属 | 说明 |
| --- | --- | --- |
| `GET /api/auth/oidc/config` | Rust compat | 返回 FerrisKey issuer、client、authorization/token endpoint、scopes、redirect config。 |
| `GET /api/auth/user` | Rust compat | 返回 `{success:true,user:{id,username}}`。 |
| `POST /api/auth/logout` 或 `/logout` | Rust compat | revoke 当前 Rust session projection。 |
| `GET /api/me` | Rust compat | 返回 roles、capabilities、device、session。 |

不再实现旧用户名密码 `/login` 直连本地账号。登录页只保留 OIDC 流程。

### 5.4 设置与 UI 偏好

| Renderer-facing 入口 | 实际归属 |
| --- | --- |
| `GET /api/settings/client?keys=...` | Rust compat -> `user_ui_preferences` |
| `PUT /api/settings/client` | Rust compat -> `user_ui_preferences` upsert |
| `PATCH /api/settings` | Rust compat |
| `GET /api/system/info` | Rust compat 聚合 Rust/Agent/RustFS/FerrisKey 摘要 |

### 5.5 桌面本地能力

| Renderer-facing 入口 | 实际归属 | 说明 |
| --- | --- | --- |
| `POST /api/shell/open-file` | desktop local | 调 Electron shell / OS default app。 |
| `POST /api/shell/show-item-in-folder` | desktop local | 本地 reveal。 |
| `POST /api/shell/open-external` | desktop local | 本地浏览器打开。 |
| `POST /api/shell/check-tool-installed` | desktop local | 检测 VSCode / terminal 等。 |
| `POST /api/shell/open-folder-with` | desktop local | 用本地工具打开工作区。 |
| `POST /api/ppt-preview/start|stop` | desktop local | OfficeCLI / watch server。 |
| `POST /api/word-preview/start|stop` | desktop local | OfficeCLI / watch server。 |
| `POST /api/excel-preview/start|stop` | desktop local | OfficeCLI / watch server。 |
| `GET /api/ppt-proxy/{port}*`, `GET /api/office-watch-proxy/{port}*` | desktop local | iframe-only loopback proxy to OfficeCLI watch server。 |

这些路由不代理到 Rust。

### 5.6 Agent、Assistant、Skill、MCP、Provider

| Renderer-facing 入口 | 实际归属 |
| --- | --- |
| `GET /api/assistants` | Rust compat，来自 `assistant_profiles + agents + capability projection` |
| `GET /api/assistants/{id}` | Rust compat |
| `POST/PUT/DELETE /api/assistants*` | Rust compat |
| `GET /api/agents/management` | Rust compat |
| `POST /api/agents/custom*` | desktop gateway facade + Rust governance；最终写 enterprise agent draft/runtime binding |
| `GET /api/skills` | Rust compat |
| `POST /api/skills/import` | Rust compat；支持本地文件/目录/ZIP、GitHub repository/tree、任意公网 HTTPS `SKILL.md`/ZIP，内容进入 enterprise skill catalog |
| `POST /api/skills/import-upload` | Rust multipart；WebUI/Electron 上传 ZIP，复用统一安全解析、授权和导入历史 |
| `GET/POST/PUT/DELETE /api/mcp/servers*` | Rust compat |
| `POST /api/mcp/test-connection` | Rust compat |
| `GET /api/providers` | Rust compat |
| `POST/PUT/DELETE /api/providers*` | Rust compat |

说明：

- `custom agent` 的配置体验可继续保留在 BiWork 页面。
- 但最终要落成 enterprise `agent / agent_version / runtime kind` 资源，而不是继续保留成未治理的本地 JSON。

### 5.7 Conversation、Run、Message、Workspace

这一组是本轮最重要的 compat 闭环，不能只做到最小 happy path。

| Renderer-facing 入口 | 实际归属 |
| --- | --- |
| `POST /api/conversations` | Rust compat |
| `POST /api/conversations/clone` | Rust compat |
| `GET /api/conversations` | Rust compat |
| `GET /api/conversations/{id}` | Rust compat |
| `GET /api/conversations/{id}/associated` | Rust compat |
| `GET /api/cron/jobs/{job_id}/conversations` | Rust compat |
| `PATCH /api/conversations/{id}` | Rust compat |
| `POST /api/conversations/{id}/messages` | Rust compat，创建 user message event + run，返回 `msg_id/turn_id/runtime` |
| `GET /api/conversations/{id}/messages` | Rust compat，回放 `conversation_messages` 或动态投影 |
| `POST /api/conversations/{id}/cancel` | Rust compat |
| `POST /api/conversations/{id}/runtime/ensure` | Rust compat，返回运行态摘要 |
| `POST /api/conversations/{id}/active-lease` | Rust compat |
| `GET /api/conversations/{id}/slash-commands` | Rust compat |
| `POST /api/conversations/{id}/side-question` | Rust compat 或明确返回 unsupported |
| `GET /api/conversations/{id}/workspace` | Rust compat |
| `GET /api/conversations/{id}/confirmations` | Rust compat |
| `POST /api/conversations/{id}/confirmations/{call_id}/confirm` | Rust compat |
| `POST /api/fs/dir|list|read|write|metadata` | desktop gateway facade -> Rust file/workbench service |
| `previewHistory.*` | desktop local 或 hybrid；不进入 Rust 控制面主线 |

说明：

1. `ensureRuntime`、`activeLease`、`slash-commands`、`turn.completed`、`conversation.listChanged` 都是 BiWork 现有页面真实依赖，不能省略。
2. `previewHistory` 是本地预览便捷能力，可继续保留为 desktop-local。
3. `fileStream.contentUpdate` 由 desktop gateway 基于 Rust 文件事件和本地保存事件进行投影。

### 5.8 Team

| Renderer-facing 入口 | 实际归属 |
| --- | --- |
| `/api/teams/*` | Rust compat -> `agent_teams` |
| `team.*` WS 事件 | desktop WS multiplexer，主体来源于 Rust compat 投影 |

Workflow DAG 页面仍然后续单开，不强塞进 team UI。

### 5.9 Cron

| Renderer-facing 入口 | 实际归属 |
| --- | --- |
| `GET /api/cron/jobs` | Rust compat -> `scheduled_jobs` |
| `GET /api/cron/jobs/{job_id}` | Rust compat |
| `POST /api/cron/jobs` | Rust compat |
| `PUT /api/cron/jobs/{job_id}` | Rust compat |
| `DELETE /api/cron/jobs/{job_id}` | Rust compat |
| `POST /api/cron/jobs/{job_id}/run` | Rust compat |
| `GET/POST/DELETE /api/cron/jobs/{job_id}/skill` | Rust compat，skill artifact / draft 映射 |
| `cron.job-*` WS 事件 | desktop WS multiplexer，主体来源于 Rust |

要求：

1. Cron job 的执行必须最终走 enterprise run gateway。
2. Cron job 里引用的 assistant/model/workspace 不能保留旧本地弱绑定，必须转成 `assistant_profile_id + agent_snapshot + model_profile_id + workspace_id`。
3. Agent 通过聊天生成 Cron 时，也要写 enterprise 审计。

### 5.10 Channel

Channel 本轮保留，但做成“两层模型”：

```text
channel management facts     -> Rust
channel connector runtime    -> desktop local capability plane
```

| Renderer-facing 入口 | 实际归属 | 说明 |
| --- | --- | --- |
| `GET /api/channel/plugins` | desktop gateway 聚合 | 聚合本地 connector 状态 + Rust policy/config 状态。 |
| `POST /api/channel/plugins/enable` | desktop local + Rust sync | 本地启用 connector，配置 secret 写 Rust secret ref 或受控配置。 |
| `POST /api/channel/plugins/disable` | desktop local + Rust sync | 同上。 |
| `POST /api/channel/plugins/test` | desktop local | connector dry-run；读 token body 或执行本地 dry-run 前必须先校验 Rust bearer session。 |
| `POST /api/channel/ingress/messages` | desktop gateway -> Rust compat | 本地 connector 收到外部消息后写 session/conversation 事实并通过 enterprise run gateway 派发。 |
| `GET /api/channel/pairings` | Rust compat | pairing 事实源。 |
| `POST /api/channel/pairings/request` | desktop gateway -> Rust compat | 本地 connector 收到外部用户绑定请求后写 Rust pairing 事实并触发 `channel.pairing-requested`。 |
| `POST /api/channel/pairings/approve|reject` | Rust compat | 审批与授权。 |
| `GET /api/channel/users` | Rust compat | authorized users。 |
| `POST /api/channel/users/revoke` | Rust compat | revoke。 |
| `GET /api/channel/sessions` | Rust compat | enterprise channel session 事实。 |
| `GET /api/channel/settings/{platform}` | Rust compat | assistant/default model binding。 |
| `PUT /api/channel/settings/{platform}/assistant` | Rust compat | 绑定 assistant profile。 |
| `PUT /api/channel/settings/{platform}/default-model` | Rust compat | 绑定 model profile。 |
| `POST /api/channel/settings/sync` | desktop gateway -> Rust sync | 触发本地 connector 与 Rust 配置对齐。 |

连接器运行要求：

1. Channel connector 不直接成为 enterprise authz 事实源。
2. 本地 connector 收到外部消息后，必须用短期 connector token 调 Rust channel ingress/internal endpoint。
3. Rust 负责把外部用户映射到 `channel_authorized_users`，再决定 assistant/default model/run 创建。
4. `channel.pairing-requested`、`channel.user-authorized`、`channel.plugin-status-changed` 由 desktop WS multiplexer 发给 renderer，其中事实主体分别来自 Rust 或本地 connector。

### 5.11 Extensions 与 Hub

Extensions / Hub 本轮保留，而且不能只做“本地插件管理”。

| Renderer-facing 入口 | 实际归属 |
| --- | --- |
| `GET /api/extensions` | desktop local，附带 Rust governance 状态 |
| `GET /api/extensions/themes` | desktop local |
| `GET /api/extensions/assistants|agents|acp-adapters|mcp-servers|skills|settings-tabs|webui` | desktop local manifest 解析 + Rust allowed filter |
| `GET /api/extensions/agent-activity` | desktop local runtime snapshot + Rust run activity projection |
| `POST /api/extensions/enable|disable` | desktop local + Rust policy/audit sync |
| `POST /api/extensions/permissions|risk-level` | desktop local manifest + Rust governance projection |
| `GET /api/hub/extensions` | desktop local hub index + Rust governance projection |
| `POST /api/hub/install|uninstall|retry-install|check-updates|update` | desktop local hub manager，完成后同步 Rust `extension_packages` |

要求：

1. Extension 的安装、解压、启停、settings tab、theme、webui static assets 继续保留在桌面端。
2. 但 extension 对 enterprise 能力的贡献必须被 Rust 看见并治理：
   - assistant
   - agent
   - skill
   - mcp server
   - channel plugin
   - acp adapter
3. `webui contributions` 可以保留，但只能作为声明式静态资源贡献：
   - 必须来自已安装且已批准的 extension
   - 必须通过 gateway 的静态资源白名单暴露
   - 不允许绕开当前认证态直接新增未治理的业务 API

### 5.12 WS 事件总线

BiWork 期望全局 `/ws`，但本轮要明确它是 desktop gateway 暴露的总线，而不是 Rust 单体 WS。

推荐协议：

1. renderer 建连 `/ws`
2. 第一帧发送：

```json
{ "op": "auth", "access_token": "..." }
```

3. gateway 校验本地 token broker 状态，并对上游 Rust enterprise WS 建立认证会话
4. renderer 后续发送：

```json
{ "op": "subscribe", "scope": "conversation", "id": "..." }
{ "op": "subscribe", "scope": "team", "id": "..." }
{ "op": "subscribe", "scope": "cron" }
{ "op": "subscribe", "scope": "channel" }
{ "op": "subscribe", "scope": "extensions" }
{ "op": "subscribe", "scope": "hub" }
```

desktop gateway 负责把两类事件合并成统一事件流：

1. Rust enterprise 事件：
   - `message.stream`
   - `turn.completed`
   - `conversation.listChanged`
   - `confirmation.*`
   - `team.*`
   - `cron.job-*`
2. local capability 事件：
   - `fileStream.contentUpdate`
   - `ppt-preview.status`
   - `word-preview.status`
   - `excel-preview.status`
   - `extensions.state-changed`
   - `hub.state-changed`
   - `channel.plugin-status-changed`

## 6. Rust 改造步骤

### 阶段 R0：收敛企业边界与 compat 合同

任务：

1. 在 `startup.rs` 增加 Rust-owned `/api` compat router，不改 `/api/v1` 合约。
2. 定义 route ownership manifest，明确哪些 `/api/*` 是 Rust enterprise 负责，哪些不是。
3. 把 run 创建逻辑从 `run_stream` SSE handler 中抽出为可复用 service，例如 `RunService::create_and_dispatch_run`。
4. compat handler 全部使用 `PlatformRequestContext`，不接受前端传 actor。
5. 对 `tenant_id` 做 compat 自动推导，旧 BiWork 请求不再要求显式传租户。

验收：

- `/api/v1/me` 仍可用。
- Rust-owned compat 路由清单可导出为文档或测试样例。
- 无 token 调 Rust-owned `/api/conversations` 返回 401。
- 旧 `/internal/*` 不受影响。

### 阶段 R1：BiWork Auth/OIDC 与设置基线

任务：

1. 新增 `/api/auth/oidc/config`、`/api/auth/user`、`/api/auth/logout`。
2. 新增 desktop/web client 配置，默认使用 `bibi-work-desktop`、`bibi-work-web`。
3. 补齐 `GET/PUT /api/settings/client` 与 `PATCH /api/settings`。
4. WS auth 侧支持 desktop gateway 上游认证、session/device last_seen 更新和 revoke 主动断链。

验收：

- BiWork 带 `Authorization: Bearer` 后可以读取 `/api/auth/user` 和 `/api/settings/client`。
- revoked session 再调用 API 失败。
- 上游 Rust enterprise WS 在 revoke 后主动断开。

### 阶段 R2：Conversation / Workbench compat 主闭环

任务：

1. 实现 assistants / providers / skills / mcp / workbench bootstrap。
2. 实现 conversation create / clone / get / list / patch。
3. 实现 send message / message history / ensure runtime / active lease / slash commands / confirmation decision。
4. 实现 workspace / file tree / preview / diff / artifact preview。
5. 在 event projection 中补 `message.stream`、`turn.completed`、`conversation.listChanged`、`confirmation.*`。

验收：

- `/guid` 能正常加载 assistant、skills、mcp、provider。
- 创建 conversation 后可进入 `/conversation/:id`。
- 发送消息会创建 Rust run，并能回放历史消息。
- `ensureRuntime` 和 `activeLease` 正常工作。
- `/slash-commands` 从 conversation/assistant 的 `available_commands`、`commands` 或 `slash_commands` 事实投影 BiWork HTTP command contract；未知字段不透传，无法识别时安全返回空数组。
- WS 事件投影覆盖 `confirmation.add/update/remove`：审批事实仍保留在 Rust approvals 表，BiWork confirmation 只作为动态 UI 投影。

### 阶段 R3：Cron 进入企业主线

任务：

1. 新增 `scheduled_jobs` 和 `scheduled_job_runs`。
2. 实现 `/api/cron/jobs*` compat。
3. 把 cron job 触发统一接到 Run Gateway 或 Workflow Gateway。
4. `cron.job-created|updated|removed|executed` 通过 WS 投影给 BiWork。
5. skill suggestion / cron trigger artifact 进入 Rust artifact / projection 主线。

验收：

- `/scheduled` 页面无红屏。
- 可创建、修改、暂停、删除、立即运行 cron job。
- cron job 触发后能创建 run，并写 audit。
- agent 生成的 cron job 与手工创建的 cron job 都能落库。

### 阶段 R4：审批、文件与 artifact

任务：

1. `approval.requested` 投影为 BiWork confirmation card。
2. `file.changed` -> `fileStream.contentUpdate` 投影。
3. `/api/fs/*` facade 所需的 file/workbench 对接补齐。
4. `ToolResultView` 的 table/chart/map/json/file_diff/artifact 映射到 BiWork preview/artifact card。
5. `expected_revision` 冲突语义固定为 409。

验收：

- high/critical tool 不审批不执行。
- 审批卡片能在 BiWork 聊天中出现。
- 文件预览保留 BiWork 现有 markdown/code/image/pdf/office/diff 样式。
- stale revision 写入返回 409。

### 阶段 R5：Channel enterprise facts 与 Extension governance

任务：

1. 新增 `extension_packages / extension_contributions / channel_*` 治理事实表。
2. 实现 channel settings / pairings / authorized users / sessions compat。
3. 实现 extension contribution sync / governance filter 接口。
4. 对 extension/channel 的 enable/disable、assistant binding、default model binding 写 audit。

验收：

- channel settings 页面能读取/写入 assistant 与 default model 绑定。
- pairing / revoke / authorized users 可以通过 Rust 主线落事实。
- extension contributions 只在被允许时出现在 BiWork 页面和 runtime snapshot。

### 阶段 R6：审计、指标和合同测试

任务：

1. 给 compat API 加 `trace_id`。
2. 对 Rust-owned `/api/*` 执行型接口全部写 authz decision 和 audit。
3. 增加 compat contract tests，固定 BiWork TypeScript 所需字段。
4. 增加 WS projection tests。

验收：

- `cargo test` 覆盖 auth、conversation send、cron、event projection、approval decision、channel settings、file preview。
- `uv run pytest` 仍通过。
- BiWork `bunx tsc --noEmit` 和关键 unit tests 通过。
- 已推进：Rust/FastAPI/Celery/Electron/ACP 已统一使用 OTLP/HTTP 与 W3C trace context；桌面 gateway proxy 覆盖完整响应生命周期，本地执行队列把 Rust 当前 context 传给 Electron ACP consumer span，再向 initialize/session/prompt 和受净化的子进程环境传播。真实 FerrisKey + Electron E2E 已确认 Rust protobuf 与 Electron JSON exporter 共享同一 trace ID，且不采集 Authorization、prompt 或响应正文。
- 已推进：observability compose 已包含 Prometheus、Alertmanager 和自动 provisioning 的 Grafana `BiWork SLO Overview`，展示 run success、10 秒内 dispatch、30 秒内 tool execution 三个 24 小时 99% SLI 及样本量；Grafana admin password、内部 metrics token 和企业 webhook 均从文件型 secret 注入。

## 7. Python agent runtime 改造步骤

### 阶段 P0：Snapshot 合同固定

任务：

1. 扩展 `RunDispatchRequest.run_config_snapshot`，固定以下字段：

```json
{
  "runtime": { "kind": "deepagents" },
  "actor": {},
  "agent": {},
  "model": {},
  "tools": [],
  "skills": [],
  "mcp_tools": [],
  "workspace": {
    "workspace_id": "uuid",
    "remote_project_id": "uuid",
    "local_mounts": []
  },
  "ui": {
    "client": "biwork",
    "conversation_type": "acp"
  }
}
```

2. Python 只读取 snapshot，不查库补权限。
3. 对缺少 `tenant_id/run_id/actor/model` 的 run 直接 fail closed 并回写 `run.failed`。

验收：

- snapshot schema 单元测试覆盖必填字段和脱敏字段。
- 运行日志不输出 secret、token、secret_ref 原文。
- 已推进：Rust `run_snapshot` 在构造本地兼容 run snapshot 和合并 published-agent 客户端 runtime namespace 前会递归剔除 `secret/token/api_key/authorization/password/secret_ref` 等敏感键；单测固定客户端 `run_config_snapshot` 中的敏感字段不会进入持久化 snapshot，同时 server-resolved `agent_id/project_id/policy_version` 仍覆盖客户端伪造值。

### 阶段 P1：BiWork 文件和 artifact 语义适配

任务：

1. Python 只输出平台事件，不输出 BiWork `IResponseMessage`。
2. `PlatformCompositeBackend` 对 `/workspace/`、`/artifacts/` 的写入带 `tool_call_id`、`expected_revision`、`reason`。
   - `/artifacts/` 在存在 tenant/project/actor 上下文时同步写入 Rust file store 并记录 revision；缺少持久化上下文时仅保留 run-scoped 内存行为。
   - artifact draft 事件按路径区分 `target.kind=artifact|workspace_file|local_file|scratch_file`，避免 BiWork 把生成 artifact 误投影为工作区文件写入。
3. `ToolResultPresenter` 对 BiWork 可渲染类型补充 `ui_hints`：`table`、`chart`、`map`、`json`、`file_diff`、`markdown`、`artifact`。
   - 已推进：Python `ToolResultPresenter` 在归一化 `ui_hints/x-ui-hints/renderer` 时保留 `title/name/label` 展示元数据，并把标题写入所有 BiWork `ToolResultView` 类型；`agent_factory -> PlatformToolWrapper -> tool.call.completed.views` 单测已固定该链路，避免工具版本声明的展示标题在 Python 端丢失。
   - 已推进：Rust `tool.call.completed.views` sanitizer 和 BiWork WS `acp_tool_call` 投影测试已固定 `ToolResultView.title` 透传，确保 Python 生成的展示标题不会在 ingest 或全局 `/ws` 投影中丢失。
   - 已推进：Rust `/api/v1/tool-result-artifacts/read` 在保留 JSON/JSONL 行分页合同的同时支持 `offset_bytes/limit_bytes` 文本 byte range，供 BiWork 对大文本 artifact 做按需读取。
   - 已推进：Rust `/api/v1/tool-result-artifacts/stream` 返回 artifact raw HTTP bytes，复用同一 object reference/文件授权边界，支持标准 `Range` 头和互斥的 `offset_bytes/limit_bytes` query；BiWork desktop gateway 已将 `/api/v1/tool-result-artifacts/*` 归类为 `rust-enterprise-api` passthrough，`httpBridge` 暴露 `httpRawRequest`，`FileService.fetchToolResultArtifactStream` 可保留 `Content-Range`/hash 等响应头供大 artifact 原始内容流式读取。
   - 已推进：BiWork `normalizeAcpToolCall` 已读取 Rust 投影到 `rawOutput.views` 的 `ToolResultView[]`，`MessageToolGroupSummary` 可展示 view 标题/kind/size，对 table 渲染静态 `<table>`，对 json/markdown/file_diff 渲染静态 `<pre>` preview；带 `data_ref/artifact_ref` 的 view 可通过 `workbench.bootstrap.auth.tenant_id + fetchToolResultArtifactRead` 按需加载 500 行/字符分页 preview，也可通过 `fetchToolResultArtifactStream` 下载 raw bytes。
   - 已推进：BiWork chart/map tool result 已接入懒加载 runtime renderer：inline-data Vega-Lite view 通过 `vega-embed` 渲染，GeoJSON map view 通过 `maplibre-gl` 的本地 GeoJSON style 渲染；带 `data.url` 等外联数据的 chart spec、加载失败或运行环境不支持 WebGL 时保留原轻量 SVG/text fallback，避免工具结果展示被 runtime 失败或外部取数阻断。

验收：

- tool result 大表写 artifact，小 preview 进 event。
- Rust 校验失败的 view 不影响工具完成事件。
- 工具执行/审计事实、模型上下文、用户展示投影必须分层；Renderer 默认展示声明式 view、友好参数和确定性结果摘要，原始 JSON 仅按需进入 Technical details。`input_summary/output_summary` 保留为治理与 fallback 字段，不作为模型结果或 UI 的唯一事实源。

### 阶段 P2：审批恢复和取消

任务：

1. 保持 `ToolRequiresApproval` + LangGraph interrupt。
2. resume worker 使用同一 `thread_id/checkpoint_id`。
3. cancel check 在 stream 循环、工具执行前后都检查。

验收：

- 多轮审批不重复执行同一个 tool_call。
- run cancel 后不会继续写文件或调用 MCP。
- 已推进：Python `PlatformCompositeBackend` 在 workspace/artifact/local/scratch 的 read/list/search/edit 入口统一先检查 cancel；取消后不会继续触达 Rust file read/list/search/write 或 local executor。
- 已推进：Python `PlatformToolAdapters` 对 MCP/local exec/SQL/第三方 Rust side-effect 调用统一做调用前后 cancel check；`PlatformCompositeBackend` 在 Rust file read/list/search/write 与 local mount executor 返回后也会再次检查 cancel，避免取消发生在外部调用期间时继续返回成功、缓存结果或发完成事件。

### 阶段 P3：本地 CLI runtime 边界

任务：

1. 固定 `runtime.kind`：

```text
deepagents   -> Python runtime
biwork_cli   -> 桌面本地 runtime
disabled     -> catalog 可见但不可运行
```

2. `biwork_cli` 不经过 Python，不复用通用 `shell` 便捷路由。
   - Python worker 在发出 `run.started` / resume `run.started` 前先拒绝显式非 `deepagents` 的 `runtime.kind`，只回写 `run.failed`。
   - Agent team 父级 `run_config_snapshot` 只持久化 Rust 生成的 team/runtime 摘要，不透传客户端原始 snapshot；成员 run 仍使用编译后的 `deepagents` snapshot 投递 Python。
   - 已推进：Rust team member 编译输入不再复制父请求完整 `run_config_snapshot`，只继承 UI/记忆/中断等安全运行上下文并强制 `runtime.kind=deepagents`；单测固定 `biwork_cli`、客户端 model/tools/secret 不会进入成员 snapshot，team 父级 summary 也不透传成员 scope/raw snapshot。
3. Rust 只负责：
   - runtime 元数据
   - assistant/agent binding
   - 审批/审计/授权
4. 实际 CLI 进程由 desktop local capability plane 托管。

验收：

- deepagents 路径不受 CLI 兼容影响。
- 未发布或未启用的 CLI agent 不可运行。
- CLI runtime 不会绕过 enterprise assistant / model / tool / audit 绑定。

## 8. BiWork 改造步骤

### 阶段 U0：共享 token broker 与 desktop gateway

任务：

1. 新增 `authTokenBroker.ts`，统一给 renderer、main process、desktop gateway 提供 access token。
2. `httpBridge.ts`、`configService.ts`、`configMigration.ts`、主进程直接 HTTP 调用全部改成走共享 token broker。
3. 保留 renderer-facing 单一 backend port，但由 desktop gateway 分流 enterprise 路由和 local capability 路由。
4. `/ws` 改为 desktop WS multiplexer，而不是 renderer 直接连 Rust。

建议配置：

```text
BIWORK_BACKEND_MODE=desktop-gateway|external-rust-direct
BIWORK_ENTERPRISE_BACKEND_URL=<rust-backend-url>
```

说明：

- `desktop-gateway` 是默认模式，才能保住 shell/office/extension/hub/channel 本地能力。
- `external-rust-direct` 只用于调试纯 enterprise compat API，不作为默认桌面运行模式。

验收：

- Electron dev 模式下仍只有一个 renderer-facing 端口。
- `httpBridge`、`configService`、主进程辅助调用都能带 token。
- `/ws` 能收本地和 Rust 两侧事件。

### 阶段 U1：OIDC 登录

任务：

1. 改造 `AuthContext.tsx`：
   - desktop 不再无条件 authenticated
   - 读取 `/api/auth/oidc/config`
   - 使用 Authorization Code + PKCE
   - token 走共享 token broker
2. `LoginPage`：
   - 保留品牌和语言选择
   - 主按钮改为 FerrisKey 登录
   - 移除“记住密码”的明文/弱混淆存储
3. `configService.initialize()` 在未登录时不阻塞 UI；登录后自动同步 `/api/settings/client`
4. WebUI 用浏览器 redirect flow；desktop 用系统浏览器 + deep link 或 loopback callback

验收：

- 未登录访问 `/guid` 跳 `/login`。
- 登录后 `/api/me`、`/api/settings/client` 可用。
- logout 后 token 清空，WS 断开。

### 阶段 U2：Conversation / Guid / Team / Cron 主流程替换数据含义

任务：

1. `/guid`：
   - assistant 列表来自 enterprise assistant profile
   - skill/MCP 勾选映射为 capability selection
   - provider/model 选择映射为 model profile
2. `/conversation/:id`：
   - 保留现有 ChatLayout、Messages、Workspace、Preview
   - 数据来自 desktop gateway -> Rust compat / local capability projection
3. `/team/:id`：
   - 映射 `agent_teams`
4. `/scheduled`：
   - 映射 enterprise `scheduled_jobs`
   - 保留现有创建、修改、run now、skill suggest 卡片

验收：

- 不重写 Layout/Sider/Preview/Messages 主体。
- `/guid`、`/conversation/:id`、`/team/:id`、`/scheduled` 可用。
- 所有新增文案走 i18n。

### 阶段 U3：Extensions / Hub / Channel 保留并治理

任务：

1. `/settings/tools`：
   - MCP server CRUD -> Rust MCP catalog
   - Tool catalog + policy -> Rust
   - 连接检查写 Rust 结构化 health；成功发现按远端权威快照同步工具，消失工具自动停用；UI 支持 enable/disable 和连续失败提示，不展示 secret 或原始后端错误正文
   - stdio 连接检查由 Electron main 使用官方 MCP SDK 无 shell 执行，结果经 authenticated `local-discovery` 回写 Rust；stdio env 只接受 `env://NAME`
   - stdio 工具调用先进入 Rust `mcp_tool` 授权边界；Rust 从目录 schema annotations 推导可信风险并按当前 actor device 写 `mcp_stdio` local-runtime work item，Electron main 只领取同设备请求并通过官方 SDK 执行
   - streamable HTTP 由 Rust 维护 initialize/session/SSE response 语义，不按普通 JSON POST 降级
2. `/settings/skills`：
   - Skill import/list/delete -> Rust Skill catalog
3. `/settings/agent` 和 `/assistants`：
   - Agent 管理与 Assistant 预设分离
4. `/settings/channels`：
   - plugin status / test / local connector enable/disable -> desktop local
   - assistant/default model binding / pairings / users / sessions -> Rust enterprise facts
5. `/settings/ext/*`、Hub 页面：
   - 扩展安装、升级、卸载、本地 enable/disable 保留
   - 贡献物列表按 Rust governance 过滤

验收：

- channel 配置页可用。
- extension / hub 页面可用。
- 被 Rust 禁用的 extension/channel contribution 不会出现在可运行配置中。

### 阶段 U4：桌面本地能力保留

任务：

1. 保留 `shell.*`。
2. 保留 `pptPreview / wordPreview / excelPreview`。
3. 保留 `previewHistory.*`。
4. 这些能力全部走 desktop local capability plane，不代理到 Rust。

验收：

- 打开文件、Reveal、Open external、Open with、Office preview 继续可用。
- WebUI 无 desktop gateway 时，这些入口会按 feature flag 正确隐藏或降级。

### 阶段 U5：仅隐藏真正不适配的能力

本轮不适配：

```text
remoteAgent
CDP remote control
真实本地远程操控
remote-agent 直连握手
```

处理方式：

1. desktop gateway / Rust compat 返回明确 feature flag。
2. BiWork 根据 `workbench.bootstrap.feature_flags` 隐藏入口。
3. 保留不影响主流程的只读 About/System/Appearance 页面。

## 9. 优先级执行清单

### M0：路由归属清单与合同基线

产物：

- 新增 `docs/biwork-api-contract.md`
- 新增 route ownership manifest
- Rust compat contract tests
- BiWork shared token broker 设计文档

验收：

- 能明确回答每一个 BiWork `/api/*` / `/ws` 事件到底归属于 Rust 还是 desktop local capability plane。
- 已推进：Rust route ownership manifest 的 router 覆盖测试已升级为 path + HTTP method 双重校验，manifest 中的 `GET/POST/PUT/PATCH/DELETE` 必须真实出现在 Axum compat router 的对应 `.route()` block 里。
- 已推进：STT visible-degrade 路由的 Rust manifest 已和实际 router / BiWork gateway 对齐：`POST /api/stt` 与 `GET /api/stt/stream` 都是 `RUST + bearer`，不再把 `/api/stt/stream` 误标成 WS start-frame。
- 已推进：BiWork `httpBridge` 合同测试已覆盖 401/403/409/500 的 Rust error envelope，确保 `BackendHttpError` 保留 `status/code/error/details/trace_id`。
- 已推进：`docs/biwork-api-contract.md` 已补齐 shared token broker 设计约束，固定 `authTokenBroker.ts` 为唯一 access token broker，并明确 HTTP、`/ws`、STT stream、logout/revoke 的 token 处理规则。

### M1：OIDC + settings bootstrap

产物：

- Rust `/api/auth/oidc/config`、`/api/auth/user`、`/api/auth/logout`
- Rust `/api/settings/client`
- BiWork AuthContext PKCE
- shared token broker

验收：

- alon 登录后看到管理能力。
- alice 登录后只能看到被授权能力。
- 登录后 `configService` 正常同步。
- 已推进：BiWork `bun run webui`、`bun run resetpass`、packaged `biwork-web resetpass` 和 Electron reset-password CLI 都在调用 legacy `/api/webui/reset-password` 前先探测 `/api/auth/status`；当后端返回 `auth_mode=ferriskey_oidc` 时会跳过或清晰失败并指向 FerrisKey/OIDC 账号恢复，避免企业 OIDC 模式下继续 seed/reset 本地密码。
- 已推进：Rust legacy `/api/auth/internal/users/system/credentials` seed endpoint 也改为 `501 + PASSWORD_AUTH_UNSUPPORTED`，即使被旧客户端直接调用也不会 silent no-op 或写入本地 WebUI 密码。
- 已推进：BiWork `authTokenBroker`、`httpBridge`、`configService` 与 renderer API/file/STT 边界都有单测或静态覆盖；未登录 bootstrap 不访问 `/api/settings/client`，token 到达后会重新同步 settings。
- 已推进：BiWork `AuthContext` DOM 单测固定 401 用户态、认证后 settings 同步、logout 后 token/session cache 清理，避免 OIDC bootstrap 与共享 token broker 脱节。
- 已推进：Electron OIDC callback 会把 access token 通过 `auth.loginCompleted` 直接交给 renderer；renderer reload 时再从 main process 恢复 token，真实 FerrisKey PKCE 登录和 reload 后 WS 重连均由 Playwright smoke 固定。
- 已验收：Rust `/api/assistants`、assistant detail 与 conversation create/clone 统一使用当前 session 的 `run:agent` 授权事实；真实 Alice FerrisKey OIDC 登录后 Guid 只显示管理员显式授权的 `LLM provider smoke agent`，不能通过列表或详情读取未授权 Agent。截图见 `19-alice-authorized-guid.png`。

### M2：desktop gateway / WS multiplexer / Guid 可用

产物：

- desktop gateway route split
- desktop WS multiplexer
- `/api/assistants`
- `/api/skills`
- `/api/mcp/servers`
- `/api/providers`
- `/api/workbench/bootstrap`

验收：

- `/guid` 无红屏。
- 可以看到 assistant/model/skills/mcp。
- `message.stream` 和本地 `extensions.state-changed` 这类事件可以共存。
- 已推进：Rust `/api/v1/workbench/bootstrap` 的强类型 `feature_flags` 也补齐 BiWork dot-path 结构，显式把 `remote_agent_direct`、`local_remote_control`、`cdp_remote_control` 置为 `false`，与 compat bootstrap 的隐藏能力合同一致。
- 已推进：Linux 本地 `services.sh` 为 BiWork desktop gateway 默认设置 `BIWORK_DISABLE_HARDWARE_ACCELERATION=1`，并记录/清理服务进程组，避免 Electron GPU fatal 或 stale PID 留下残余 Vite/gateway 进程。
- 已推进：Rust 全局 `/ws` 已按 BiWork `subscribe/unsubscribe` 维护连接级 scope/id 订阅集合；未订阅不推业务事件，conversation/team/cron/channel/extensions/hub 事件只发送给匹配订阅，避免 renderer 收到越 scope 的 enterprise event。
- 已推进：Rust 全局 `/ws` 已把 session/device 存活校验从心跳拆成 5 秒周期 refresh；发现 session/device revoked 或 token expired 时发送 `auth.revoked` 并关闭连接。
- 已推进：Rust conversation-scoped `/api/v1/conversations/{conversation_id}/events/stream` 和旧 `/api/v1/conversations/{conversation_id}/ws` 也复用同一 session/device refresh；SSE 发送 `auth.revoked` 后结束流，旧 WS 发送 `{ "type": "auth.revoked" }` 后关闭。
- 已推进：BiWork `httpBridge` 与 WebUI `browser` adapter 已把 Rust `auth.revoked/auth.failed` 视为终端认证事件，收到后清理 shared token、关闭当前 WS 并禁止自动重连，避免已撤销 session/device 被前端重连策略重新打到后端。
- 已推进：WebUI `browser` adapter 在 WS 认证成功后会主动订阅 Rust enterprise scopes：conversation、team、cron、channel、extensions、hub；这补齐了 Rust 订阅过滤生效后 WebUI 浏览器模式收不到业务事件的断点。
- 已推进：真实 Electron + Rust backend 的 Playwright smoke 已验证 `/guid` 无红屏、WS 收到 `auth.ok`，并覆盖 renderer reload 后 token 恢复与重新连接；Rust OIDC device fingerprint 已改为绑定稳定 session key，修复同一 token 从不同 `User-Agent` 访问时 session `device_id` 漂移、WS 误收 `auth.revoked` 的问题，审批页和定时任务页 reload 均有回归断言。

### M3：Conversation run 主闭环

产物：

- `/api/conversations/{id}/messages`
- `/api/conversations/{id}/messages` history
- `/api/conversations/{id}/workspace` file tree/search
- `/api/conversations/{id}/runtime/ensure`
- `/api/conversations/{id}/active-lease`
- `/api/conversations/{id}/slash-commands`
- `/ws` 的 conversation 事件投影

验收：

- 发送一条消息后，Python deepagents 执行，BiWork 消息区流式显示。
- 刷新页面后可回放历史消息。
- 已推进：Rust `run.failed` -> BiWork `message.stream` error 投影会携带 `rawError.traceId`，BiWork `chatLib` 会保留该诊断字段，避免流式失败消息在进入聊天消息模型后丢失后端 trace 关联。
- 已推进：BiWork `getWorkspace(search)` 已对齐 Rust `/api/conversations/{id}/workspace?search=...` 返回的 `full_path/relative_path`，并同步触发旧 `responseSearchWorkSpace` 搜索响应事件，保留工作区搜索 UI 的单一命中/计数合同。
- 已推进：BiWork `ipcBridge.conversation` 合同测试已固定 `sendMessage`、`activeLease`、`slash-commands` 和 paged message history 的路径/query/body；`docs/biwork-api-contract.md` 同步明确 history 返回 cursor page，不再误写成裸数组。
- 已推进：BiWork 新建 conversation 时会钉住所选 Agent 的最新 published AgentVersion，send path 会把该版本交给可信快照编译；真实 Playwright smoke 已验证绑定 ToolVersion 出现在 run snapshot，不再退化为只有 `agent_id`、`agent_version_id=null` 的无版本运行。
- 已推进：Rust WS 与历史消息投影对同一 run 使用稳定 `assistant.{run_id}` 身份；`message.completed` 以 `replace=true` 覆盖流式 delta，历史回填会丢弃已被完成事件取代的同 run delta，避免刷新或实时/历史合并后重复显示最终回复。
- 已推进：Python event normalizer 以流式状态机过滤 `<think>/<thinking>/<analysis>/<reasoning>` 内部推理段，`message.delta`、`message.completed.content` 与公开 result 投影都不再泄露 reasoning；平台 file tool completion 通过 `ui_tool_call_id` 复用 LangGraph stream tool identity，BiWork 将 started/delta 与治理后的 terminal view 合并为同一工具卡，不再显示重复步骤。

### M4：Cron

产物：

- `/api/cron/jobs*`
- `scheduled_jobs`
- `cron.job-*` 事件

验收：

- `/scheduled` 页面可创建、编辑、立即执行任务。
- cron 触发能进入 run gateway。
- `cron.job-executed` 投影保留 `cron_job_id`、`conversation_id`、`run_id`、`triggered_at`，前端订阅不会丢失 enterprise run 事实。
- 已推进：Rust `/api/conversations/{id}/associated` 不再固定空数组；会按 conversation metadata 的 `cron_job_id/cronJobId` 或 `scheduled_jobs.source/target_conversation_id` 找到对应 cron job，并复用 `/api/cron/jobs/{job_id}/conversations` 的 BiWork conversation 投影返回关联会话。
- 已推进：BiWork `ipcBridge.cron` 合同测试固定 list/create/update/delete/run-now 路径与 payload；`runNow` 不再把 path-only `job_id` 误作为请求体发送，`updateJob` 在只更新 `execution_mode`、没有 `target.payload` 时也不会崩。
- 已推进：BiWork 默认 Manual 任务会以 `schedule.kind=cron`、空表达式创建，Rust 将其视为只允许手动执行的合法任务；`at/every` 仍要求非空表达式。
- 已推进：Create Scheduled Task 保存成功后主动 refetch job list，不再依赖可能延迟或漏失的 WS create event 才显示新卡片。
- 已验收：真实 Electron/FerrisKey Playwright 从 `/scheduled` 页面创建 Manual job，点击 Run now 后生成独立 conversation/run；run 使用 published AgentVersion、最终 `completed`、零 approval，页面只显示一条 `ok` 且 Processing 消失，测试 finally 软删除 job/conversation。截图见 `04-scheduled.png` 与 `14-cron-run-now.png`。

### M5：审批、文件、shell / Office preview

产物：

- `approval.requested` -> `confirmation.add`
- `/api/fs/*`
- `fileStream.contentUpdate`
- local shell routes
- local office preview routes

验收：

- 高风险工具卡片出现在 BiWork。
- 已推进：BiWork 会话消息区除启动恢复 pending confirmations 外，也订阅实时 `confirmation.add/update/remove`；新增审批请求会立即生成 permission 卡片，审批状态变化按 `id/call_id` 更新，审批结束按 `id/call_id` 清理，避免只能刷新后出现高风险工具卡或卡片内容滞后。
- 已推进：pending confirmation 恢复把服务端列表视为权威快照，会同时补入漏失的 add 并移除漏失的 remove；3 秒轮询、focus/visibility 和实时事件均可触发恢复，请求进行中收到的新触发会排队再执行一次，避免因并发合并而永久漏掉最新状态。
- 已推进：ToolVersion 的 `risk_level/requires_approval` 会进入 Python runtime snapshot；Rust tool authorize 在同一 `run + resource + args_hash` 的已批准调用被 LangGraph resume 重放时原子复用原 tool call，避免生成第二个孤儿 pending approval。真实 Playwright smoke 已完成 `high risk -> confirmation.add -> Allow once -> Python resume -> HTTP tool status=ok -> run completed`，并确认该 run 仅 1 条 approval、0 条 pending approval、tool call 为 completed。
- 已推进：审批决定事务会解析对应 open interrupt；conversation cancel/delete 即使遇到已经 completed 的 run，也会关闭该 conversation（可选 turn）遗留的 pending approval、open interrupt 和 waiting-approval tool call。Playwright 通过待审批时 reload 模拟漏失实时事件，验证权威恢复后仍可 Allow once 并完成 run。
- 已推进：Rust 审计归档增加独立 archive worker；BiWork 产生的审批、工具和授权 evidence 在写入 RustFS 前由 Rust 递归脱敏并限流，sealed segment 经 object hash/manifest 校验后才进入 archived 状态和 retention 周期，renderer 不承担任何归档可信性判断。
- 已推进：Rust 增加 tenant/segment/resource legal hold、整分区 retention eligibility、UTC 月度 audit partition、分区预创建 worker、安全 detach/drop 和历史 hash backfill API。BiWork 仍只消费业务能力，不在 renderer 判断“是否允许清理”；混合 hash chain、active hold、未到期、未验证归档、默认分区或包含任意不合格记录的叶子分区均由 Rust fail closed。cleanup 默认 dry-run，要求 `platform_admin`，服务端执行开关默认关闭。
- 文件树、预览、diff、下载、复制路径可用。
- 已推进：Rust 文件搜索已从单行 `file_search_documents` 改为 `file_search_chunks`；小文本全量分块，大文本均匀采样头/中/尾并返回最佳命中 chunk 作为 `content_truncated=true` preview，bibi_work_frontend/desktop gateway 不应因搜索结果自动 hydrate 整个大对象；后台回填 worker 会周期扫描缺失/损坏 chunk 的历史 revision，并从 inline/RustFS 内容幂等重建索引。
- `/api/fs/write` 对 Rust-backed workspace/artifact/scratch 路径保留 `expected_revision` 语义，stale write 返回 `409 + WORKSPACE_REVISION_CONFLICT`。
  - 已推进：Rust `/api/fs/metadata` 返回 `revision/etag`；BiWork Preview tab 保存 Rust-backed 文件时把 tab revision 作为 `expected_revision` 回传，保存成功后刷新 revision metadata。
- `/api/fs/metadata`、`/api/fs/image-base64` 的 BiWork 最小合同已写入 `docs/biwork-api-contract.md`；Rust compat 单测固定 `expected_revision` 同时接受 number 和 `rev_<number>`，避免 Preview 保存 token 形状漂移。
- `/api/fs/image-base64` 对 Rust-backed 图片返回完整 `data:<content-type>;base64,...`，保持 BiWork 预览组件合同。
- 已推进：BiWork `PreviewContext` 单测已固定 `fileStream.contentUpdate` 的前端消费语义：agent 写入事件会防抖更新匹配的 clean preview tab，但不会覆盖用户未保存的 dirty tab。
- shell / Office preview 不依赖 Rust enterprise 路由仍可工作。
  - 已推进：BiWork `ipcBridge` 本地能力合同测试已固定 `shell.*`、`document.convert`、`ppt/word/excel-preview` 的 POST path/body 和 preview status 事件名，避免后续误改成 Rust enterprise 路由或丢失 workspace 参数。

### M6：Extensions / Hub / Channel

产物：

- extension governance sync
- hub install/uninstall/update 保留
- channel plugin facade
- pairings / users / sessions / assistant binding / default model binding

验收：

- channel 页面可配置。
  - 已推进：BiWork `ipcBridge.channel` 的 settings/plugin adapter 增加合同测试，固定 `/api/channel/settings/{platform}` 的读写路径、`assistant_id`/`default_model` 请求体，以及 `/api/channel/settings/sync` 返回的 connector 状态映射。
  - 已推进：Rust `/api/channel/plugins/enable|disable` 写操作返回最新 plugin status；BiWork channel 设置页直接用写操作结果更新状态，减少二次列表拉取并固定 REST 写后读合同。
  - 已推进：desktop gateway 的 `/api/channel/plugins/test` 在读取包含 connector token 的请求体或执行本地 dry-run 前先校验 Rust bearer session；Rust route ownership manifest 也将该本地 dry-run 标记为 `auth=bearer`，不再把它描述成裸 desktop-session 入口。
  - 已推进：Rust `/api/channel/pairings/approve|reject` 与 `/api/channel/users/revoke` 返回结构化 decision/revocation contract；BiWork channel 表单据此局部更新 pending pairing 与 authorized users 列表。
  - 已推进：Rust `/api/channel/pairings*`、`/api/channel/users*` 和 `/api/channel/sessions` 已接入显式 route authz；真实 Postgres 回归固定 pairing approve / user revoke / session list 在授权拒绝时不会写 pairing、authorized user 或 session 事实。
  - 已推进：Rust `/api/channel/ingress/messages` 响应抽成可测 contract；BiWork `ipcBridge.channel.ingressMessage` 暴露类型化入口，供本地 connector 复用同一 run gateway 请求/响应形状。
  - 已推进：Rust channel ingress 的 fail-closed 顺序已用 compat 单测固定：必须先按 enabled connector + active authorized user 解析绑定，之后才能创建/复用 session、创建 conversation 或 dispatch run。
  - 已验收：真实 Electron/FerrisKey Playwright 启用 Telegram connector、创建 pairing request，BiWork Channels 页面实时显示 pairing code；点击 Approve 后切换为 Authorized Users，Rust 返回授权用户事实；测试 finally revoke 用户并 disable connector。截图见 `15-channel-pairing-requested.png`、`16-channel-user-authorized.png`。
- hub / extension 页面可用。
  - 已推进：Agent 管理页已接回 Agent Hub 入口，打开后使用受治理的 `/api/hub/extensions` 数据；没有可安装项时显示稳定空态，而不是永久 loading 或不可达死代码。
- hub install / retry-install / uninstall / update 的本地动作完成后会触发 extension manifest sync，把 `extension_packages` / `extension_contributions` 治理事实同步回 Rust。
  - 已推进：desktop gateway 已实现真实 Hub installer：支持 HTTP(S) 与受大小限制的 base64 data tarball，强制 SHA-512 SRI 校验，拒绝不安全 extension name，受限解压并校验 `aion-extension.json` name，使用 staging + backup 做原子安装/更新，卸载只删除受控 install root 下的目标目录。
  - 已推进：desktop gateway 的 extension sync payload 合并 `hub-local-state.json`，安装/失败/卸载状态都会同步 package/device facts；Rust 对同一 hub package 的后续本地 sync 会保留 catalog `dist/integrity` 元数据，避免卸载后丢失再次安装来源。
  - 已推进：hub install / retry-install / uninstall / update 的 desktop gateway 响应会附带 `governanceSync` 摘要，前端调用者能确认本地动作之后 Rust extension governance 已完成同步。
  - 已推进：Rust `/api/extensions` 读模型透出 `installed/install_status/installError`，与 `/api/hub/extensions` 共享设备安装状态，避免 extension 页面看不到 hub 同步后的失败或未安装状态。
  - 已验收：真实 Electron Hub 页面从 Install 切换为 Installed；本地目录完成受校验解压，Rust `/api/extensions/acp-adapters` 出现 extension contribution。调用卸载后重新加载页面恢复 Install，contribution 被治理过滤。截图见 `17-extension-installed.png`、`18-extension-uninstalled.png`。
- extension/channel contribution 会受到 enterprise governance 约束。
  - 已推进：desktop gateway 聚合 `/api/channel/plugins` 时使用 Rust `/api/extensions/channel-plugins` 作为 extension channel plugin allow-list，缺少运行态条目时也以 Rust 允许的 contract 为权威，仅从本地 manifest 补充展示元数据。
  - 已推进：`/api/extensions/static/{extension}/{asset}` 在读取本地 extension 文件前会用 Rust `/api/extensions` 的 installed/enabled allow-list 校验；未同步或被治理过滤的 extension 不能通过直连静态 URL 读取资源。
  - 已推进：BiWork `ipcBridge.extensions` 合同测试已固定 `/api/extensions` 聚合读模型、settings tabs、agent activity、enable/disable、permissions、risk-level、i18n 的 REST path/body，以及 `extensions.state-changed` 事件名，避免 renderer 把 governance 操作改成 path-encoded 本地直连。
  - 已推进：Rust extension contribution SQL 过滤已补真实 Postgres 回归，固定 contribution/device/package 三层治理条件：contribution enabled、device installed/enabled/install_status=installed、package status 允许，以及 `webui` contribution 必须 package approved。
  - 已推进：Rust run snapshot 在 SQL governance 过滤之外增加 runtime contribution 白名单防线，只投影 `assistant/agent/skill/mcp_server/channel_plugin/acp_adapter` 给 Python runtime，并用单测固定 `theme/settings_tab/webui` 不会进入 `extension_contributions`。

### M7：Team 与 DAG 后续增强

产物：

- `/api/teams/*`
- `team.*` 事件投影
- team member run 状态展示

验收：

- team 多 agent run 能在 BiWork team 页面看到状态。
  - 已推进：`team.run.updated` WS 投影保留 Rust payload 中的有效 BiWork run status（如 `cancelling`），避免把中间态误覆盖成 `running`。
  - 已推进：BiWork team run REST/state 读模型也保留 `accepted/cancelling/cancelled` 等 TeamRunStatus；Rust 派生 team run 状态时不再用 member `running` 覆盖服务端 `cancelling`。
  - 已推进：team cancel 请求写入 `cancelling` 中间态并发布 `team.run.updated` / `team.member.updated`；Python runtime 回传 `run.completed|failed|cancelled` 后由 Rust 聚合生成 `team.member.*` 和 `team.run.*` 终态事件。
  - 已推进：BiWork `useTeamRunView` 将 `completed/cancelled/failed` 终态保留为 `lastTerminalRun` 并在 team slot header 展示状态徽标，避免 WS 终态事件到达后页面立即清空导致用户看不到结果；sendbox 仍只依据 active run/slot work 判定是否锁定。
  - 已推进：BiWork `ipcBridge.team` 合同测试已固定 `activeLease`、team broadcast send、slot-targeted send、run cancel、child cancel、slot pause 的 REST path/body，避免 path-only `team_id/team_run_id/slot_id` 泄漏进请求体或误路由。
- DAG workflow 仍走 enterprise `/api/v1/workflow-*`。
  - 已推进：Rust route ownership manifest 和 BiWork desktop gateway classifier 已显式标记 `/api/v1/workflow-*` 为 `RUST` / `rust-enterprise-api` passthrough，并用 Rust/前端单测固定 workflow DAG 不会被归入 `/api/teams/*` compat 或 desktop local capability plane。

## 10. 可验证性与测试策略

### 10.1 每个阶段必须给出的验证证据

每个阶段都必须至少提供四类证据：

1. API / WS 合同样例：
   - request sample
   - response sample
   - error sample
2. 自动化测试：
   - unit / integration / contract
3. 越权或失败路径：
   - 401
   - 403
   - 409
   - revoke / cancel / approval retry
4. UI 快照：
   - `agent-browser` 截图
   - 或 Playwright 截图

没有这四类证据，不算阶段完成。

### 10.2 Rust

```text
cargo fmt
cargo check
cargo test biwork_compat
cargo test agent_platform::authz::tests
DATABASE_URL=... REDIS_URL=... cargo test biwork_conversation_send_roundtrip -- --ignored
DATABASE_URL=... REDIS_URL=... cargo test biwork_cron_roundtrip -- --ignored
DATABASE_URL=... REDIS_URL=... cargo test biwork_channel_binding_roundtrip -- --ignored
DATABASE_URL=... RUSTFS_ENDPOINT=... cargo test biwork_workspace_preview_roundtrip -- --ignored
```

重点补充：

- OIDC/JWKS middleware/verifier 集成回归已补：mock discovery/JWKS、roles claim、kid/key refresh、expired/nbf/audience/issuer/azp 失败路径
- create-run / workflow-run / cron-run API handler 已补 capability deny 回归
- public authz check/batch-check actor mismatch 审计归属、run snapshot actor 防伪造、Python tool wrapper deny/review 二次鉴权、平台工具 resource 映射、critical/high risk 策略、AgentVersion SQL binding -> runtime `sql_tools` snapshot、写/DDL SQL critical fail-closed、SQL env-backed read execute、MCP env-backed discover/execute 和第三方 HTTP env-backed execute 回归已补
- Rust secret data plane 已统一为 `env://`、Vault KV 和 KMS decrypt gateway；MCP/HTTP/SQL/LLM 共用 resolver，SQL TLS 支持 verify CA/mTLS，LLM rotate/revoke 会撤销已签发 runtime credentials。BiWork 只展示 `has_secret_ref/scheme/available`，不读取或缓存 secret value。
- 已推进自动 credential rotation：BiWork 模型设置页已接入 health/policy，展示 worker/gateway 健康、凭证策略、下次轮换和失败提示；worker/gateway 未配置时启用操作 fail closed。renderer 不接收 current/new secret ref、引用 hash 或网关错误正文，attempt API 仅供受控诊断查询。
- channel pairing / revoke / session route authz 回归已补
- extension governance filter 回归已补

### 10.3 Python

```text
uv run ruff check .
uv run pytest
uv run pytest bibi_work_agent/tests/test_deepagents_hitl.py
uv run pytest bibi_work_agent/tests/test_runtime_dag_e2e.py
```

重点补充：

- snapshot schema 必填/脱敏、`sql_tools` list 校验和绑定 SQL runtime tool 回归已补
- deepagents cancel
- approval resume
- `runtime.kind=deepagents`
- 对 `biwork_cli` 非 Python runtime 的 fail closed 回归

### 10.4 BiWork

```text
bunx tsc --noEmit
bun run test -- tests/unit/common-adapter/httpBridge.test.ts
bun run test -- tests/unit/common-adapter/browserRealtimeError.test.ts
bun run test -- tests/unit/renderer/messageListStreaming.dom.test.tsx
bun run test -- tests/unit/previews/PreviewPanel.dom.test.tsx
```

重点补充：

- shared token broker tests
- configService unauthenticated bootstrap tests
- desktop gateway route split tests
- channel / extensions / hub settings page unit tests

### 10.5 `agent-browser` 快照验证

必须把 `agent-browser` 明确写进执行流程，不能只停留在“手工看看页面”。

建议流程：

1. 启动 desktop gateway 或 WebUI host
2. 用 `agent-browser` 连接服务
3. 分别打开并截图：
   - `/login`
   - `/guid`
   - `/conversation/:id`
   - `/settings/tools`
   - `/settings/skills`
   - `/scheduled`
   - `/settings/channels`
   - `/settings/ext/*`
4. 与改造前基线截图对比：
   - 布局是否偏离
   - 导航是否断裂
   - 核心交互是否消失
   - 文案/i18n 是否错位
5. 将截图和偏离分析作为里程碑验收附件

如果阶段涉及 Preview、Workspace、Cron、Channel、Extension 页面，而没有截图比对，不算完成。

2026-07-10 已增加 `bibi_work_frontend/tests/e2e/specs/enterprise-live-smoke.e2e.ts`，通过真实 Electron CDP、FerrisKey PKCE、Rust WS 和 Python deepagents 执行 Alon/Alice 双用户登录、资源授权可见性、read/write/low-risk tool、high-risk tool 审批续跑、Manual cron 创建/Run now、Hub install/uninstall、channel pairing/authorize 与主导航 smoke。截图保存在 `artifacts/playwright/2026-07-10/`，覆盖 login、FerrisKey、Guid、conversation stream、approval requested/resumed、Scheduled 创建成功、cron run-now 完成、Team、Agent、Tools、Skills、Channels、channel pairing/authorized user、Hub install/uninstall，以及 Alice 仅见授权 Agent 和三工具完成态；已确认这些页面无红屏、主导航未断裂，审批卡和 Team 三栏稳定，cron 完成回复没有实时/历史重复，extension contribution 只在 installed 状态可见，模型 reasoning 不进入可见消息，三个工具只显示一个 `View Steps · 3` 分组。

### 10.6 端到端 smoke

1. FerrisKey 登录 alon。
2. 创建 / 发布 AgentVersion，绑定 read_file / write_file 和一个 low-risk tool。
3. 授权 alice 运行。
4. Alice 在 `/guid` 选择 assistant，创建 conversation。
5. 发送消息，看到 stream、tool group、file preview。
6. 触发 high-risk tool，审批卡出现。
7. Alon 批准，run resume completed。
8. 创建 cron job 并 run now，确认生成 enterprise run。
9. 配置一个 channel assistant binding，完成至少一次 pairing / authorize 路径 smoke。
10. 安装或启用一个 extension，确认贡献物出现在页面且经过 governance 过滤。
11. 校验 audit hash chain verify 通过。

2026-07-10 已完成步骤 1 至 11 的真实 smoke：Alon 通过 FerrisKey 登录并创建 published AgentVersion，精确绑定 `read_file`、`write_file` 和 low-risk HTTP tool；管理员通过临时 policy bindings 授权 Alice `run:agent`、`use/execute:tool`。Alice 使用独立 FerrisKey OIDC session 登录后，Guid 只显示该授权 Agent；run 依次完成 artifact write/read 与 health tool，无审批卡，历史事件包含三类工具和 artifact path，最终只显示 `alice smoke ok`。同一套测试还覆盖 high-risk ToolVersion 审批卡、Allow once、同 checkpoint resume、单次执行、run completed、零孤儿 pending approval；Manual cron Run now 生成独立 completed enterprise run；Telegram pairing request -> Approve -> Authorized Users；受校验 Hub extension install/uninstall 与 contribution governance；audit hash chain verify 返回 `valid=true`。测试 finally 会删除/软删除会话与 job、撤销 channel 用户、禁用 connector、卸载 Hub fixture、禁用临时 AgentVersion 与 policy bindings；本地 Hub 状态和数据库测试事实在测试后由清理步骤核验为空。

## 11. 风险与处理

| 风险 | 处理 |
| --- | --- |
| 把 BiWork 所有 `/api/*` 误认为都应由 Rust 提供 | 先写 route ownership manifest，再开始改代码。 |
| OIDC 只改 `httpBridge`，遗漏 `configService` 和主进程直连请求 | 统一 token broker，禁止新增裸 `fetch('/api/*')`。 |
| desktop 直连 Rust 后，shell / office / extensions / hub / channel 本地能力失效 | 默认模式必须是 `desktop-gateway`，`external-rust-direct` 只用于调试。 |
| WS 事件来源多，消息乱序或漏订阅 | desktop WS multiplexer 统一做 auth、subscribe、upstream replay、local event merge。 |
| assistant 绑定表设计过弱，后续 SQL 难维护 | 使用归一化 `assistant_profile_capability_bindings`。 |
| 审批表被 UI 投影污染 | confirmation shape 只存在于 projection/cache，不写审批核心事实表。 |
| Cron 继续当本地功能做，后续审计和授权断裂 | 统一落到 enterprise `scheduled_jobs` 与 run gateway。 |
| Extension / Hub / Channel 只保留本地安装，不做企业治理 | 本地安装保留，但 manifest / contribution / binding / pairings / audit 必须进 Rust。 |
| shell / Office preview 被误压入 Rust 控制面 | 明确它们属于 desktop local capability plane。 |
| compat、Electron gateway 或 IPC bridge 再次膨胀成单体模块 | 按高变更率业务子域拆分 handler、DTO mapper 和 adapter；只提取稳定的认证、错误、追踪等窄公共能力，不建立通用 CRUD framework。 |
| 阶段性验证记录持续堆入新文档 | 当前状态和剩余工作只更新本执行方案；接口事实更新 API 合同；长期架构决策更新架构文档；逐次测试数字留在提交、CI 或测试产物。 |

当前仍需持续治理的架构债务包括：Rust compat/WS/workflow/memory 等热点模块继续按业务边界收敛，Electron entry 与 IPC bridge 继续拆分生命周期和业务 adapter，补齐 DAG 可视化编辑器，并以真实负载验证长连接、Team/DAG、conversation 列表和大对象容量。模块是否继续拆分应以职责、依赖方向、变更频率和测试隔离为依据，不以文件行数单独决策。

## 12. 最小首轮落地建议

首轮不要试图一次性适配 BiWork 全部功能。建议按下面顺序提交：

1. 路由归属清单 + Rust compat 基线 + desktop gateway 设计定稿。
2. shared token broker + OIDC + `/api/settings/client`。
3. desktop gateway WS multiplexer + `/api/assistants`、`/api/providers`、`/api/skills`、`/api/mcp/servers`、`/api/workbench/bootstrap`。
4. conversation create / send / history / runtime ensure / active lease / stream。
5. cron compat 与 scheduled run。
6. approval projection、file preview、`/api/fs/*`。
7. shell / Office preview 路由保留到 desktop local capability plane。
8. extensions / hub / channel 的治理与同步。

这样能最快验证五件关键事：

1. BiWork 能登录。
2. BiWork 能拉到 enterprise 配置和工作台数据。
3. BiWork 能创建并运行 enterprise run。
4. shell / Office preview 等桌面本地能力没有在重构中丢失。
5. cron / channel / extensions / hub 没有被粗暴下线，而是被正确收进平台边界。

## 13. 附录：Route Ownership Manifest

本附录只给实现阶段快速查表使用。具体字段合同仍以 [biwork-api-contract.md](./biwork-api-contract.md) 为准。

### 13.1 Prefix Summary

| 路由前缀 | Ownership | 说明 |
| --- | --- | --- |
| `/api/auth/*` | Rust | OIDC/session projection |
| `/api/me` | Rust | 当前用户上下文 |
| `/api/settings*` | Rust | UI preference 事实源 |
| `/api/assistants*` | Rust | assistant profile compat |
| `/api/agents/management` | Rust | managed agent catalog |
| `/api/agents/custom*` | Aggregate | 本地 runtime 编辑体验 + Rust governance |
| `/api/skills*` | Rust | skill catalog |
| `/api/mcp/*` | Rust | MCP catalog |
| `/api/providers*` | Rust | provider/profile compat |
| `/api/workbench/*` | Rust | Guid/workbench bootstrap 与 feature gating |
| `/api/conversations*` | Rust | conversation/run 主线 |
| `/api/teams*` | Rust | team 主线 |
| `/api/fs/*` | Facade | gateway 包装 -> Rust file/workbench service；进入本地上传、浏览、快照、watch、zip、preview-history 前必须先校验 Rust bearer session |
| `/api/cron/*` | Rust | scheduled jobs |
| `/api/channel/plugins` | Aggregate | local connector runtime + Rust config/policy |
| `/api/channel/plugins/test` | Local | local connector dry-run，不代理到 Rust；读 token body 或执行本地 dry-run 前必须先校验 Rust bearer session |
| `/api/channel/plugins/enable`, `/api/channel/plugins/disable` | Rust | enterprise enable/disable governance and audit |
| `/api/channel/pairings*` | Rust | pairing 事实 |
| `/api/channel/users*` | Rust | authorized users |
| `/api/channel/sessions*` | Rust | enterprise session 事实 |
| `/api/channel/settings*` | Rust | assistant/default model binding |
| `/api/extensions*` | Aggregate | local extension runtime + Rust governance |
| `/api/hub/extensions` | Aggregate | local hub index + Rust governance projection |
| `/api/hub/install`, `/api/hub/uninstall`, `/api/hub/retry-install`, `/api/hub/check-updates`, `/api/hub/update` | Local | hub install/update manager；执行本地状态读写前必须先校验 Rust bearer session |
| `/api/shell/*` | Local | desktop convenience routes |
| `/api/ppt-preview/*` | Local | Office preview |
| `/api/word-preview/*` | Local | Office preview |
| `/api/excel-preview/*` | Local | Office preview |
| `/api/ppt-proxy/{port}*`, `/api/office-watch-proxy/{port}*` | Local | Office watch iframe loopback proxy |
| `/ws` | Aggregate | desktop WS multiplexer |

### 13.2 实施规则

1. 新增 renderer-facing 路由前，先把它登记到 route ownership manifest。
2. 任何人如果不能回答“这个路由最终归 Rust、Local、Aggregate、Facade 哪一类”，就不要先写实现。
3. `execution-plan`、`biwork-api-contract.md`、实际 gateway 路由表三者必须同时更新。
