# 企业级智能体运行平台 Rust/Python 后端开发文档

更新日期：2026-06-22

## 1. 文档目标

本文档面向 Rust 后端、Python agent runtime 和智能体编排实现，目标是把 `docs/enterprise-agent-platform-architecture.md` 中的架构原则落成可开发、可验证、可分阶段交付的后端工程方案。

当前不设计前端页面，不生成前端代码。前端只需要依赖本文档中的 API、事件、状态机和权限语义，等后端接口稳定后再实现。

成功标准：

1. Rust 端形成唯一可信控制面：认证、租户、资源级授权、Agent/Skill/MCP/Tool 管理、Run Gateway、事件、审批、文件、记忆、工作流、审计全部从 Rust 进入。
2. Python 端形成受控运行面：只接收 Rust 下发的 run/config snapshot，使用 deepagents 执行，所有文件和工具副作用回调 Rust 二次鉴权。
3. FerrisKey 作为身份、用户、角色、scope、token 的事实源；Rust 作为资源级授权 PEP/PDP 适配层。
4. RustFS 作为对象存储和版本化文件事实源之一，但不是权限边界；所有访问必须经过 Rust File/Object Service。
5. 审计、审批、运行事件和工具调用形成闭环，能够解释“谁在什么角色下对什么资源做了什么、为何允许或拒绝”。
6. 工具结果支持结构化展示协议：摘要用于审计和折叠预览，表格、图表、地图、JSON、文件 diff 和 artifact 通过 Rust 校验后的 `ToolResultView` 渲染，不允许工具或 LLM 直接注入任意前端代码。

## 2. 明确假设与待确认点

### 2.1 本文档采用的假设

1. `alon` 是平台管理员账号，`alice` 是普通用户账号；两者的初始密码只属于本地 FerrisKey 初始化信息，不应写入代码、迁移或运行配置。
2. 需求中“ferriskey 中定义的 users”后面列出的 `tenant_member`、`workflow_operator`、`tool_user_low` 等按 FerrisKey roles 解释，不按用户账号解释。
3. 当前 FerrisKey 已提供 OIDC、realm、client、user、role、scope、token 能力；资源级 `/api/v1/authz/check` 与 `/api/v1/authz/batch-check` 不作为当前阶段 FerrisKey 已有能力假设。
4. 第一阶段由 Rust 后端实现本地 resource authz adapter，后续可以把 adapter 的后端替换为 FerrisKey 资源级 PDP、OpenFGA 或其他策略引擎，但外部 API 不变。
5. Python 端使用 deepagents 作为 agent harness，但 deepagents permissions、HITL、backend、checkpointer 只是运行态防护和恢复机制，不是企业安全边界。
6. 本地执行能力以后由 Tauri local executor 承担；当前后端文档只定义 Rust 与 local executor 的协议边界，不实现前端。
7. 认证改造已按破坏性更新执行：Rust 端不兼容旧本地 JWT 签发、Claims 结构和本地 `/auth/login` 语义，目标状态直接切到 FerrisKey OIDC access token + Rust 本地 session/device 投影。

### 2.2 需要后续确认的问题

1. FerrisKey roles 是否已有稳定 role id。如果没有，v1 先使用 role name，后续增加 `ferriskey_role_projection` 映射稳定 id。
2. `alon` 是否需要同时拥有 `tenant_admin`、`agent_admin`、`skill_admin`、`mcp_admin`、`audit_admin`、`security_admin` 等管理员角色。如果 FerrisKey 中尚未创建这些角色，建议补齐。
3. `alice` 是否只是普通用户，还是同时具备某些部门角色，例如教务、图书馆、班主任。本文档按“普通用户 + 需要时授予部门/能力角色”设计。
4. v1 是否允许 Python runtime 直接访问模型供应商密钥。建议不允许，模型密钥应由 Rust 端 secret ref 下发，Python 只看到运行期解析后的最小必要配置。

## 3. 当前仓库状态摘要

### 3.1 Rust 已有基础

当前 `bibi_work_backend` 已具备以下雏形：

- Axum API：`/api/v1` 与 `/internal` 分离。
- AuthN：受保护 API 已切到 FerrisKey OIDC access token 校验；`/api/v1/auth/oidc/config` 暴露客户端所需 issuer、audience、authorization endpoint、token endpoint 和 JWKS URI。
- FerrisKey token 校验：Rust middleware 校验 issuer、audience、signature、exp、nbf、kid/JWKS，并解析 `sub`、`preferred_username`、`email`、`realm_access.roles` 或 `roles`、`jti`、`sid/session_state`、`exp`。
- 平台投影：Rust 按 FerrisKey token upsert `platform_users`、`devices`、`platform_sessions`，并把 `platform_user_id`、`tenant_id`、roles、`session_id`、`device_id` 传给业务 handler。
- 当前用户上下文：`GET /api/v1/me` 已返回当前平台用户、tenant membership、roles、UI capabilities、device 和 session 摘要。`capabilities` 只用于前端显示和功能开关，实际授权仍以后端 `ResourceAuthzService` 为准。
- 业务表：tenant、device、session、agent、agent_version、skill、tool、mcp、project、conversation、run、run_event、outbox、tool_call、approval、interrupt、audit、file_revision、memory、workflow、local_exec。
- 接口模型：`AuthzCheckRequest`、`AuthzDecision`、`CreateRunRequest`、`ToolAuthorizeRequest`、`FileWriteRequest`、`CreateWorkflowRunRequest` 等。
- 本地资源授权：`ResourceAuthzService` 使用 Postgres tenant membership、resource relations、policy bindings 和 FerrisKey roles 做资源级 allow/review/deny 判定。
- Run Gateway：`POST /api/v1/conversations/{conversation_id}/runs:stream` 创建 run，并派发到 Python runtime。
- Internal event ingest：`POST /internal/run-events` 写入 `run_events`。
- 工具二次鉴权入口：`POST /internal/tool-calls:authorize`。
- LLM 管理与凭证注入：provider/model profile/credential 已有基础管理接口，统一以 `ResourceResponse` 返回；所有 public response 中的时间字段必须序列化为 RFC3339 string，满足前端 contract。credential 响应只返回脱敏状态，不返回 `secret_ref` 原文；Rust 在 run dispatch 前解析 env-backed LLM secret，签发短期 `runtime_credential_id`，并通过 `/internal/runtime-credentials/{runtime_credential_id}` 供 Python 按 tenant/run scope 获取。
- 文件接口：`/internal/files/read|write|edit|list|glob|search` 面向 Python/runtime；`/api/v1/projects/{project_id}/files|files/read|files:search|files/history|artifacts` 面向前端只读查询，public wrapper 固定使用当前 FerrisKey actor，不接受浏览器伪造 actor。`read` 已支持默认最新版、指定 `revision` 和指定 `version_id` 读取，`glob` 已支持基础 `*`/`?` pattern；对象存储启用时内容主路径写 RustFS，禁用时保留测试/降级用 inline fallback。
- 审批入口：`GET /api/v1/approvals`、`POST /api/v1/approvals/{approval_id}/decision`。
- 初始化脚本：`bibi_work_backend/scripts/bootstrap_ferriskey.py` 幂等初始化 FerrisKey realm/client/roles/users，并同步 Postgres tenant、platform users、memberships、role projection、resource relations 和 policy bindings。

主要缺口：

- 前端/桌面端尚未在仓库内实现 FerrisKey OIDC redirect/token flow；后端已经不再提供本地 `/auth/login`。
- session/device 查询、撤销和 logout API 已补齐；middleware 已维护投影并对本地 revoked device/session fail closed。真实 FerrisKey token 已通过 `/api/v1/tenants` smoke test；管理员多端管理视图和 WebSocket revoke 断开仍缺。
- FerrisKey bootstrap 脚本已在真实本地 FerrisKey 实例上验证，能幂等创建 realm/client/roles/users 并同步 Postgres 投影。当前限制是 FerrisKey 已分配的 realm roles 尚未进入 access token 的 `realm_access.roles`，Rust 当前通过 Postgres membership/role projection 完成资源授权。
- 文件写入已接入 RustFS/object_store 边界：`/internal/files/write|edit` 在 `object_store.enabled=true` 时由 Rust File Service 写入 `bibi-work-files`，并写入 `object_references`、`file_revisions.object_reference_id`、bucket/key、etag、version_id、hash、content_type 和二进制/大对象 metadata；读取对象内容时会做 sha256 完整性校验，二进制对象默认只返回 metadata，需要显式 `allow_binary=true` 才返回 `content_base64`；对象写成功但数据库持久化失败会尽力删除孤儿对象；`object_store.enabled=false` 时仍保留测试/降级用 inline fallback。RustFS client put/get/delete/get_opts、File Store + Postgres + RustFS 历史 revision 回归、File Service HTTP + RustFS E2E 已通过本地服务验证，E2E 已覆盖 `expected_revision` 冲突、file lock 冲突/释放、二进制对象、大对象和文件写入审计证据；新增 `file_search_documents` Postgres tsvector 索引，文本且非大对象文件写入时同步索引，`/internal/files/search` 和 public `files:search` 优先走索引；`/internal/files/list|glob|search` 与 public file list/search 已返回虚拟目录 `entries`。当前限制是本地 RustFS/object_store PUT 未返回 `version_id` 时只能验证平台 `revision` 历史读取，按 `version_id` 读取能力依赖底层返回并记录 version id；生产级归档策略仍缺。
- 工作流 run 已能展开 `workflow_node_runs` 和依赖，并通过 internal tick 创建节点 agent run；workflow design/version/run 已补 public list/get/detail/node-runs/validate 读取面；retry/cancel/timeout、run/node 终态保护、plan 内并发限制、节点 agent 权限编译、AgentVersion 绑定 skill/tool/MCP 逐项使用授权、基础 input/output mapping、Rust -> Python cancel 标记传播、数据库级 DAG 回归、能力授权真实 Postgres 回归和 Python runtime 三节点 DAG payload E2E 已推进。仍缺阻塞中 agent 调用强取消、更复杂 mapping 表达式、能力授权真实 API 回归，以及 Rust scheduler -> 真实 Python service 的跨进程 DAG E2E。
- 记忆已有 `memory_items` CRUD、候选 activate/reject/archive、候选批处理治理、core_profile/procedural 初始候选约束、跨用户/跨项目隔离回归、基础关键词检索、向量检索元数据、内部 retrieve-for-run、内部 candidates/access-log、检索脱敏、访问审计日志、基础 indexing worker、真实 Qdrant indexing E2E、archive/reject 后 Qdrant 删除回归、Conversation Run Gateway 和 workflow node run 的 memory context snapshot 注入；`run.completed` 已能基于 `memory_candidates` payload 自动生成 candidate，Python runtime 已能从结构化结果和明确 memory candidates 区块中保守提取候选。仍缺前端治理台和更细管理员治理策略。
- 审计 hash chain 已接入当前授权审计、工具事件完成/失败、文件对象写入和 file lock 状态变化路径：`audit_logs.prev_hash/row_hash` 由 Rust 端统一生成，按 tenant 串链，并用 Postgres 事务级 advisory lock 防止同租户并发写入分叉；`GET /api/v1/audit/hash-chain:verify` 已支持按 tenant 重算最近 N 条已哈希审计行并报告断链位置；`POST /api/v1/audit/hash-chain:seal` 已支持从上一段 segment 后继续封存未 seal 审计链，生成 segment manifest，并在 RustFS 启用时写入独立 `audit_bucket` 证据对象和 `object_references`；应用启动已支持可配置自动定期 sealing worker；审批决策已归档 `approval_evidence` 对象并回填 `approvals.evidence_object_reference_id`；high/critical tool-call completed/failed 事件会归档摘要化 `tool_call_evidence` 对象并回填 `tool_calls.evidence_object_reference_id`。仍缺历史审计回填、脱敏归档策略和分区归档。

### 3.2 Python 已有基础

当前 `bibi_work_agent` 已具备以下雏形：

- FastAPI internal API：`/internal/agent-runs`、`/internal/agent-runs/{run_id}/resume`。
- Celery worker：执行 run、resume run；worker app 显式 include `bibi_work_agent.workers.tasks`，避免 API 进程可入队但 worker 任务表为空。
- Rust 回调客户端：`/internal/run-events`、`/internal/tool-calls:authorize`、`/internal/runtime-credentials/{runtime_credential_id}`、`/internal/files/*`。
- deepagents 调用入口：`create_deep_agent(...)`。
- 模块拆分：`api`、`clients`、`runtime`、`backends`、`tools`、`workers` 已从单文件运行态中拆出。
- 事件与回调：已新增 `RustClient`、`EventEmitter`、`AgentEventNormalizer`。
- 工具控制：已新增 `PlatformToolWrapper`、`classify_tool_risk`、`tools/io_policy.py` 和 `ToolResultPresenter`，工具执行前先调用 Rust `/internal/tool-calls:authorize`，输入摘要和工具输出会按授权 obligations 做基础脱敏与截断，工具 completed/failed 会以 run event 回写 Rust；completed 事件已可携带 preview 级 `ToolResultView[]`。

主要缺口：

- `PlatformCompositeBackend` 已实现基础虚拟路径隔离、workspace Rust read/write/list/search 回调和 scratch 存储/检索；scratch list/search 已补虚拟目录 `entries`，workspace 目录语义由 Rust File Service 统一返回；仍需按 deepagents backend 协议补全更完整的二进制、memory/policy backend。
- `PlatformCheckpointer` 已在原有 Postgres 持久化原语之上适配 LangGraph `BaseCheckpointSaver` 基础协议；普通 run 和 resume 均会显式传入 `configurable.thread_id`。Python runtime resume API 已改为 Celery 入队，resume worker 使用 `Command(resume=...)` 续跑，并用 `approval_id` 做 Redis 幂等防重；已补真实 deepagents graph + HITL interrupt + approve resume 回归，验证高风险工具审批后只执行一次；仍需更完整的 checkpoint saver 性能/裁剪策略。
- streaming 事件已有 normalizer 骨架，但还需要补完整事件覆盖、幂等和回归测试。
- 工具结果已具备结构化视图主链路：Python 可从 list/dict/string/显式 Vega-Lite spec/GeoJSON 和规范化 `ui_hints` 生成 table/json/markdown/chart/map view，大表会通过 internal file API 写入 JSONL artifact 并生成 object reference；Rust ingest 会过滤非法 `views`，对 artifact object reference 做 tenant/run/hash/content-type 校验，并把通过校验的引用登记到 `tool_result_artifacts`；前端已有 TanStack Table preview 分页、artifact 500 行分页、轻量虚拟滚动、Vega-Lite、MapLibre preview 和 artifact 内容按需读取。仍缺跨进程真实 run E2E 和对象存储 range/streaming reader。
- LLM runtime credential 获取已接入：Python runtime 会按 `tenant_id/run_id/runtime_credential_id` 调 Rust internal endpoint 取回短期凭证，并只注入模型调用所需的 `api_key`，不把 secret 写入持久化配置。OpenAI-compatible provider 通过 `langchain-openai` 构造 ChatOpenAI；`base_url` 由用户配置并原样透传，不自动补 `/v1` 或改写 `/v3` 等版本路径。
- MCP/local exec/SQL/第三方工具 wrapper 已接入统一 `PlatformToolWrapper`：local exec 只创建 Rust `/internal/local-exec/requests`，不在 Python spawn；MCP/SQL/第三方工具只调用 Rust internal 受控 endpoint，不让 agent runtime 持有 MCP secret、数据库密码、第三方 URL/header secret。Rust 已新增 MCP HTTP JSON-RPC adapter、注册 SQL read tool executor 和第三方 HTTP executor；secret-backed MCP/SQL/HTTP 工具在 resolver 未实现前仍 fail closed。ToolWrapper 已能发 completed/failed 事件，Rust ingest 已反写 `tool_calls.status/output_summary/error_summary/completed_at`、写入工具执行审计 hash-chain 行，并对 high/critical 工具调用归档 `tool_call_evidence`；Python wrapper -> RustClient HTTP request/serialization 回归、真实 Rust internal HTTP 服务级工具事件 E2E、真实 deepagents/HITL approval continuation 回归和执行器 ignored E2E 均已通过。
- Celery 幂等、取消、超时、重试去重、run 状态回写还不完整。

## 4. 总体工程边界

```text
Client / Future Frontend
  -> Rust Backend API
       AuthN/session/device
       Resource Authz Adapter
       Agent/Skill/MCP/Tool Control Plane
       Run Gateway
       Event Ingest + Outbox
       File/Object Service
       Memory Service
       Approval Service
       Workflow Scheduler
       Audit Service
       Local Exec Coordinator
  -> Python Agent Runtime only through Rust internal dispatch
       FastAPI internal API
       Celery worker
       deepagents harness
       Platform backends
       Tool wrappers
       Event normalizer
```

关键边界：

- 用户端不直连 Python、Redis、RustFS、local executor、MCP server。
- Python 不持有平台资源权限事实，不直接访问 FerrisKey 做资源级最终判定。
- Rust 是所有副作用入口的 PEP。所有 allow/review/deny 决策都写审计。
- Redis 只做实时投递和短期续传；Postgres 是 run event 和审计事实源。
- RustFS 只存对象内容、运行产物、记忆文件、审计证据；权限、索引、元数据由 Rust/Postgres 管理。

## 5. 角色与资源授权设计

### 5.1 FerrisKey roles 规划

已有或需求中出现的角色：

```text
tenant_member
dept_academic_affairs_member
dept_academic_affairs_approver
dept_library_member
dept_library_approver
class_advisor
class_advisor_approver
agent_runner
workflow_operator
skill_user
mcp_user
tool_user_low
tool_user_medium
personal_space_owner
```

建议补齐管理员角色：

```text
platform_admin
tenant_admin
security_admin
audit_admin
agent_admin
skill_admin
mcp_admin
tool_admin
workflow_admin
memory_admin
project_admin
local_exec_admin
```

本地账号建议：

| 用户 | 定位 | 推荐角色 |
| --- | --- | --- |
| `alon` | 平台管理员 | `platform_admin`, `tenant_admin`, `security_admin`, `audit_admin`, `agent_admin`, `skill_admin`, `mcp_admin`, `tool_admin`, `workflow_admin`, `memory_admin`, `project_admin`, `local_exec_admin` |
| `alice` | 普通用户 | `tenant_member`, `agent_runner`, `workflow_operator`, `skill_user`, `mcp_user`, `tool_user_low`, `personal_space_owner`，按业务再加部门角色 |

### 5.2 资源级权限原则

FerrisKey roles 只做粗粒度入口和可见性输入。实例级权限放在 Rust/Postgres：

```text
resource_relations
- tenant:{id}#admin@user:{id}
- tenant:{id}#member@user:{id}
- department:{id}#member@user:{id}
- project:{id}#owner@user:{id}
- project:{id}#member@user:{id}
- agent:{id}#runner@role:{role_name}
- skill:{id}#user@role:{role_name}
- tool:{id}#user@role:{role_name}
- mcp_tool:{server_id}:{name}#user@role:{role_name}
- workflow:{id}#operator@role:{role_name}
- approval:{id}#approver@user:{id}
- device:{id}#owner@user:{id}
```

策略绑定独立存储：

```text
resource_policy_bindings
- id
- tenant_id
- resource_type
- resource_id
- action
- subject_type: user|role|relation
- subject_id
- effect: allow|deny|review
- risk_level: low|medium|high|critical
- obligations
- policy_version
- created_by_user_id
- created_at
- disabled_at
```

### 5.3 Rust 授权接口

公共接口：

```http
POST /api/v1/authz/check
POST /api/v1/authz/batch-check
```

内部接口：

```http
POST /internal/authz/check
POST /internal/authz/batch-check
```

请求：

```json
{
  "tenant_id": "uuid",
  "actor": {
    "user_id": "uuid",
    "device_id": "uuid|null",
    "session_id": "uuid|null"
  },
  "action": "execute",
  "resource": {
    "type": "tool",
    "id": "uuid-or-stable-name",
    "path": null
  },
  "context": {
    "project_id": "uuid|null",
    "conversation_id": "uuid|null",
    "run_id": "uuid|null",
    "workflow_run_id": "uuid|null",
    "agent_id": "uuid|null",
    "tool_id": "uuid|null",
    "mcp_server_id": "uuid|null",
    "args_hash": "sha256|null",
    "risk_level": "low|medium|high|critical",
    "source_ip": "string|null",
    "user_agent": "string|null"
  }
}
```

响应：

```json
{
  "decision": "allow|deny|review",
  "policy_version": "local-policy-v1",
  "reason_code": "role_missing|relation_missing|risk_requires_review|null",
  "obligations": {
    "approval_policy_id": "policy-id|null",
    "approval_timeout_sec": 3600,
    "audit_level": "normal|high|critical",
    "redact_fields": ["secret", "token"],
    "max_output_bytes": 1048576,
    "require_mfa": false
  }
}
```

实现建议：

- v1 当前由 `ResourceAuthzService` 直接读取 FerrisKey roles claim/session roles snapshot + Postgres memberships/relations/policy bindings。
- 授权服务本地失败时 fail closed，public/internal authz handler 负责把决策写入 `authz_decisions` 和 `audit_logs`。
- 不再保留空实现的外部 FerrisKey PDP 占位代码；未来 FerrisKey 支持资源级 PDP 后，应新增清晰的 adapter/trait 边界再切换。

## 6. Rust 端模块设计

当前 `features/agent_platform/handlers.rs` 已收敛为 handler facade，事件/outbox、文件修订存储、workflow plan/runtime/compile、run、approval、memory、workflow scheduler 和多类 catalog/service 编排已拆到子模块。当前实际模块包括：

```text
src/features/agent_platform/
  event_store.rs      # run_events/event_outbox/Redis publish/SSE replay
  file_store.rs       # virtual path 校验、file_revisions 读写、file.changed 事件
  workflow_compile.rs # workflow plan agent_version 校验、节点权限/能力快照编译
  workflow_plan.rs    # compiled_plan 解析、node/edge/DAG 校验
  workflow_runtime.rs # workflow/node 终态、状态聚合和 retry 判定纯函数
  handlers/
    run_service.rs          # run 创建、事件 ingest、SSE/WS 回放、outbox publish
    approval_service.rs     # tool authorize、approval list/decision、resume callback
    memory_service.rs       # memory CRUD、候选治理、检索、索引入队
    workflow_scheduler.rs   # workflow design/version/run、cancel、tick 调度
    tenant_session_service.rs
    project_service.rs
    file_service.rs
    authz_service.rs
    local_exec_service.rs
    llm_catalog_service.rs
    mcp_catalog_service.rs
    skill_tool_catalog_service.rs
    agent_catalog_service.rs
    support.rs
```

截至 2026-06-21 核对，主 `handlers.rs` 为 53 行 facade；较重模块主要是 `memory_service.rs` 约 2686 行、`workflow_scheduler.rs` 约 1727 行、`run_service.rs` 约 1031 行、`support.rs` 约 586 行。当前已经把主 HTTP 文件拆开，但 memory、workflow、run 和 support 仍混合了 DTO/SQL/状态机/测试/审计辅助，尚未达到最终高内聚目标。

后续建议继续按高内聚模块拆分：

```text
src/features/agent_platform/
  mod.rs
  api/
    agents.rs
    skills.rs
    tools.rs
    mcp.rs
    projects.rs
    conversations.rs
    runs.rs
    approvals.rs
    memories.rs
    workflows.rs
    files.rs
    local_exec.rs
    authz.rs
  application/
    authz_service.rs
    run_service.rs
    event_service.rs
    approval_service.rs
    file_service.rs
    memory_service.rs
    workflow_scheduler.rs
    audit_service.rs
    agent_catalog_service.rs
  domain/
    authz.rs
    run.rs
    event.rs
    approval.rs
    file.rs
    memory.rs
    workflow.rs
    audit.rs
  infrastructure/
    ferriskey_oidc.rs
    resource_authz_local.rs
    rustfs_client.rs
    redis_stream.rs
    agent_runtime_client.rs
    outbox_publisher.rs
    local_exec_gateway.rs
  repository/
    agents.rs
    authz.rs
    events.rs
    files.rs
    memories.rs
    workflows.rs
    audit.rs
```

分层规则：

- `api` 只做 HTTP extraction、DTO 校验、调用 service、响应转换。
- `application` 编排事务、状态机、审计和外部调用。
- `domain` 放状态机、策略枚举、事件类型和不可变规则。
- `infrastructure` 放 FerrisKey、RustFS、Redis、Python runtime、local executor 适配。
- `repository` 只处理 SQL，不做权限判断。

## 7. Rust API 契约

### 7.1 Auth/session/device

目标状态是 FerrisKey-only。Rust 不再签发用户访问令牌，不再兼容当前本地 JWT。Rust 只接受 FerrisKey `bibi-work` realm 签发的 OIDC access token，并在本地维护 session/device 投影用于多端登录、设备撤销、审计和本地执行绑定。

已破坏性废弃：

- 旧本地 `/api/v1/auth/login` 直接签发 access/refresh token 的语义。
- 旧本地 `Claims` 中以 Rust 自签 JWT 为事实源的 `iss/sub/sid/device_id/jti` 结构。
- 任何把本地 JWT 当作平台身份事实源的中间件和测试。

保留的概念不是本地 JWT，而是本地会话投影：

```text
FerrisKey access token
  -> Rust verifies issuer/audience/signature/expiry/roles
  -> Rust upserts or checks platform_sessions/devices
  -> RequestContext {
       tenant_id,
       ferriskey_subject,
       platform_user_id,
       roles,
       session_id,
       device_id,
       token_jti,
       token_exp
     }
```

当前已实现：

```http
GET  /api/v1/auth/oidc/config
GET  /api/v1/me
POST /api/v1/devices
GET  /api/v1/devices
POST /api/v1/devices/{device_id}/revoke
GET  /api/v1/sessions
POST /api/v1/sessions/{session_id}/revoke
POST /api/v1/auth/logout
```

当前登录链路约束：

1. 客户端或未来前端通过 FerrisKey OIDC 完成登录和 token 获取。
2. 调用 Rust API 时使用 `Authorization: Bearer <FerrisKey access token>`。
3. Rust 不处理用户密码，不签发本地 access/refresh token，不提供本地 `/auth/login` 兼容路径。
4. Rust middleware 每次请求校验 FerrisKey token，并 upsert 本地 `platform_users`、`devices`、`platform_sessions` 投影。
5. 业务 handler 只能使用 middleware 注入的 `PlatformRequestContext`，不能信任客户端请求体中的 actor roles、session 或 device。

后续仍需补齐前端/桌面端 OIDC redirect/token flow，以及 session/device revoke 后主动关闭既有 SSE/WebSocket 连接。

Rust token/session 校验必须包含：

- FerrisKey issuer、audience、signature、kid/JWKS、exp、nbf、jti。
- `realm_access.roles` 或统一 `roles` claim。
- FerrisKey `sub` 到本地 `platform_users.id` 的稳定映射。
- `platform_sessions` 未撤销。
- `devices` 未撤销。

当前目标表结构：

```text
platform_users
- id
- tenant_id
- ferriskey_subject
- username
- email
- display_name
- status
- created_at
- updated_at

platform_sessions
- id
- tenant_id
- user_id
- device_id
- ferriskey_subject
- ferriskey_session_state
- token_jti
- token_exp
- roles_snapshot
- token_hash
- source_ip
- user_agent
- revoked_at
- created_at
- updated_at
```

`platform_sessions` 不保存 FerrisKey access token 明文。需要诊断时只保存 `token_jti`、`token_exp`、roles snapshot 和 hash。

设备投影至少包含：

```text
devices
- id
- tenant_id
- user_id
- device_fingerprint
- device_name
- platform
- trust_level
- last_seen_at
- revoked_at
- created_at
- updated_at
```

### 7.2 Agent/Skill/Tool/MCP/LLM 管理

当前控制面已补齐基础创建、列表、读取、PATCH、软禁用、版本列表/读取/发布/禁用、AgentVersion 绑定、AgentVersion validate/effective-capabilities、MCP tools discover/list/get/update/disable、LLM credential list/create/revoke 和策略绑定管理入口。LLM runtime credential 已有 env-backed resolver 最小闭环。仍未完成的是 credential rotate、LLM model profile test、Vault/KMS resolver、secret-backed MCP discover、OpenAPI/TS schema 生成、discover 差异审计和更细的只读可见性过滤。

```http
POST /api/v1/agents
GET  /api/v1/agents
GET  /api/v1/agents/{agent_id}
PATCH /api/v1/agents/{agent_id}
POST  /api/v1/agents/{agent_id}/disable
GET  /api/v1/agents/{agent_id}/versions
POST /api/v1/agents/{agent_id}/versions
GET  /api/v1/agent-versions/{agent_version_id}
POST /api/v1/agent-versions/{agent_version_id}/disable
POST /api/v1/agent-versions/{agent_version_id}/bindings
GET  /api/v1/agent-versions/{agent_version_id}/effective-capabilities
POST /api/v1/agent-versions/{agent_version_id}/validate

POST /api/v1/skills
GET  /api/v1/skills
GET  /api/v1/skills/{skill_id}
PATCH /api/v1/skills/{skill_id}
POST  /api/v1/skills/{skill_id}/disable
GET  /api/v1/skills/{skill_id}/versions
POST /api/v1/skills/{skill_id}/versions
GET  /api/v1/skill-versions/{skill_version_id}
POST /api/v1/skill-versions/{skill_version_id}/disable

POST /api/v1/tools
GET  /api/v1/tools
GET  /api/v1/tools/{tool_id}
PATCH /api/v1/tools/{tool_id}
POST  /api/v1/tools/{tool_id}/disable
GET  /api/v1/tools/{tool_id}/versions
POST /api/v1/tools/{tool_id}/versions
GET  /api/v1/tool-versions/{tool_version_id}
POST /api/v1/tool-versions/{tool_version_id}/disable

POST /api/v1/mcp-servers
GET  /api/v1/mcp-servers
GET  /api/v1/mcp-servers/{mcp_server_id}
PATCH /api/v1/mcp-servers/{mcp_server_id}
POST  /api/v1/mcp-servers/{mcp_server_id}/disable
GET  /api/v1/mcp-servers/{mcp_server_id}/tools
POST /api/v1/mcp-servers/{mcp_server_id}/tools
POST /api/v1/mcp-servers/{mcp_server_id}/tools:discover
GET  /api/v1/mcp-tools/{mcp_tool_id}
PATCH /api/v1/mcp-tools/{mcp_tool_id}
POST  /api/v1/mcp-tools/{mcp_tool_id}/disable

POST /api/v1/llm-providers
GET  /api/v1/llm-providers
GET  /api/v1/llm-providers/{provider_id}
PATCH /api/v1/llm-providers/{provider_id}
POST  /api/v1/llm-providers/{provider_id}/disable
POST /api/v1/llm-credentials
GET  /api/v1/llm-credentials
POST /api/v1/llm-credentials/{credential_id}/revoke
POST /api/v1/llm-model-profiles
GET  /api/v1/llm-model-profiles
GET  /api/v1/llm-model-profiles/{profile_id}
PATCH /api/v1/llm-model-profiles/{profile_id}
POST  /api/v1/llm-model-profiles/{profile_id}/disable
```

策略绑定统一接口：

```http
GET  /api/v1/policy-bindings?resource_type=agent&resource_id={id}
POST /api/v1/policy-bindings
POST /api/v1/policy-bindings/{binding_id}/disable
```

仍未实现的生命周期接口：

```http
POST  /api/v1/llm-credentials/{credential_id}/rotate
POST  /api/v1/llm-model-profiles/{profile_id}/test
```

所有管理接口先校验：

- `tenant:{tenant_id}#manage` 或对应 admin role。
- 对资源实例的 `manage` 动作。
- 写 audit。

#### 7.2.1 Agent 生命周期与能力绑定

Agent 是平台中的“可运行能力单元”，DAG workflow node 是“对某个 AgentVersion 的一次任务调用”。Workflow 不隐式创建 Agent；管理员先创建并发布 AgentVersion，Workflow 设计阶段只能选择已发布且当前 actor 有 `run/use` 权限的 AgentVersion。

推荐生命周期：

```text
1. alon 创建 agent draft
2. 配置 system_prompt、LLM profile、默认文件权限、记忆策略、风险策略
3. 绑定 skill_versions、tool_versions、mcp_tools、subagent specs
4. 配置可见性和运行权限 policy bindings
5. 发布 immutable agent_version
6. alice 创建 conversation run 或 workflow node run 时引用 agent_version_id
7. Rust 创建 run_config_snapshot，Python 只按 snapshot 执行
```

Agent draft 可以编辑，AgentVersion 不可变。运行时永远引用 AgentVersion，不引用 mutable draft。

#### 7.2.2 绑定模型

已有绑定表继续保留，并补充绑定策略和顺序：

```text
agent_version_skill_bindings
- agent_version_id
- skill_version_id
- load_order
- injection_mode: system|developer|context|tool_hint
- required: bool

agent_version_tool_bindings
- agent_version_id
- tool_version_id
- default_enabled
- permission_profile_id
- risk_override

agent_version_mcp_bindings
- agent_version_id
- mcp_tool_id
- default_enabled
- permission_profile_id
- risk_override

agent_version_subagent_bindings
- agent_version_id
- subagent_key
- child_agent_version_id
- description
- tool_scope: inherit|explicit
- max_iterations
```

绑定规则：

- `skill_version` 只能提供 prompt、知识、流程约束或工具使用建议，不能绕过工具授权。
- `tool_version` 是平台内置或自定义工具的 schema/version 快照。
- `mcp_tool` 来自 MCP discover 结果，必须保留 server、tool name、schema hash。
- `subagent` 建议绑定到另一个已发布 AgentVersion，而不是复制 prompt。这样专家智能体可以独立演进、审计和授权。
- 发布 AgentVersion 时必须固化 `schema_hash`、`skill content_hash`、`mcp schema_hash`、`policy_version`。

绑定接口建议细化：

```http
PUT  /api/v1/agent-versions/{agent_version_id}/skills
PUT  /api/v1/agent-versions/{agent_version_id}/tools
PUT  /api/v1/agent-versions/{agent_version_id}/mcp-tools
PUT  /api/v1/agent-versions/{agent_version_id}/subagents
GET  /api/v1/agent-versions/{agent_version_id}/effective-capabilities
POST /api/v1/agent-versions/{agent_version_id}/validate
```

`effective-capabilities` 返回发布后 Python runtime 能看到的最终快照，不返回密钥明文。

#### 7.2.3 LLM、模型参数、Token 与凭证

LLM 配置不应散落在 Agent draft 的任意 JSON 中。建议作为一等资源管理：

```http
POST /api/v1/llm-providers
GET  /api/v1/llm-providers
GET  /api/v1/llm-providers/{provider_id}
PATCH /api/v1/llm-providers/{provider_id}
POST  /api/v1/llm-providers/{provider_id}/disable
POST /api/v1/llm-credentials
GET  /api/v1/llm-credentials
POST /api/v1/llm-credentials/{credential_id}/revoke
POST /api/v1/llm-model-profiles
GET  /api/v1/llm-model-profiles
GET  /api/v1/llm-model-profiles/{profile_id}
PATCH /api/v1/llm-model-profiles/{profile_id}
POST  /api/v1/llm-model-profiles/{profile_id}/disable
```

仍未实现：

```http
POST /api/v1/llm-credentials/{credential_id}/rotate
POST /api/v1/llm-model-profiles/{profile_id}/test
```

推荐表：

```text
llm_providers
- id
- tenant_id
- provider_key: openai|anthropic|google|openai-compatible|ollama|custom
- display_name
- base_url: 原样保存和透传，不自动补 `/v1` 或改写用户提供的版本路径
- auth_scheme: bearer|api_key_header|none
- default_headers_template
- status
- created_at

llm_credentials
- id
- tenant_id
- provider_id
- owner_scope: tenant|department|user|agent
- owner_resource_id
- secret_ref
- secret_hash
- expires_at
- rotation_status
- created_by_user_id
- created_at
- revoked_at

llm_model_profiles
- id
- tenant_id
- provider_id
- credential_id
- profile_name
- model_name
- context_window
- max_input_tokens
- max_output_tokens
- temperature
- top_p
- reasoning_effort
- response_format
- tool_choice_policy
- rate_limit_policy
- cost_policy
- status
- created_at
```

凭证规则：

- API key、OAuth token、refresh token、database password、MCP secret 只保存到 Vault/KMS 或 RustFS 加密 secret bundle 的 `secret_ref`，不进普通 Postgres 明文字段。
- Postgres 只保存 `secret_ref`、`secret_hash`、过期时间、轮换状态和审计信息。
- Rust 可以在调度 run 时解析 secret ref，并只把 Python 运行所需的最小凭证以短期方式注入 runtime；日志和事件不得记录 secret ref 原文或 secret value。
- 当前已实现最小 resolver：仅支持 `env://NAME` / `env:NAME` 的 LLM credential，在普通 run、workflow node run 和 approval resume 下签发 10 分钟 Redis `runtime_credential_id`，Python runtime 通过 internal `/internal/runtime-credentials/{runtime_credential_id}` 按 `tenant_id/run_id` 取回短期凭证。非 env/Vault/KMS scheme 当前 fail closed。
- public LLM Provider/Profile/Credential 响应使用通用 `ResourceResponse`，具体字段位于 `metadata`；`created_at/updated_at` 必须是 RFC3339 string。credential metadata 只包含 `has_secret_ref` 等脱敏状态，不包含 `secret_ref`。
- 所有 token 输出、错误、审计摘要必须脱敏：`authorization`、`bearer`、`api_key`、`token`、`secret`、`password`。
- 对工具执行可签发短期 capability token，绑定 `actor/resource/action/policy_version/expires_at/jti`，只用于一次文件、本地执行或对象访问，不等同于用户登录 token。

AgentVersion 引用 LLM：

```json
{
  "model_profile_id": "uuid",
  "model_overrides": {
    "temperature": 0.2,
    "max_output_tokens": 4096
  },
  "fallback_model_profile_ids": ["uuid"]
}
```

运行时 `run_config_snapshot` 必须固化：

- `model_profile_id`
- provider/model/base_url 摘要
- credential `has_secret_ref` 和短期 `runtime_credential_id`，不包含 `secret_ref` 原文
- model 参数
- token/cost/rate limit policy version
- Python runtime 会把 openai/openai-compatible 的模型配置转换为 LangChain chat model；`base_url` 按快照原值传入，不做版本路径推断。

#### 7.2.4 SQL 工具定义

SQL 不应作为“任意字符串工具”直接暴露给 agent。平台应把 SQL 连接和 SQL 工具定义成受控资源。

接口：

```http
POST /api/v1/sql-connections
GET  /api/v1/sql-connections
POST /api/v1/sql-connections/{connection_id}/test
POST /api/v1/sql-tools
GET  /api/v1/sql-tools
POST /api/v1/sql-tools/{sql_tool_id}/versions
POST /api/v1/sql-tools/{sql_tool_id}/validate
```

推荐表：

```text
sql_connections
- id
- tenant_id
- name
- database_kind: postgres|mysql|sqlite|mssql|other
- host
- port
- database_name
- username_ref
- password_secret_ref
- tls_config_ref
- allowed_schemas
- allowed_tables
- max_rows
- statement_timeout_ms
- status

sql_tool_versions
- id
- tenant_id
- sql_tool_id
- connection_id
- version_label
- operation: read|write|ddl
- parameter_schema
- sql_template
- query_hash
- allowed_roles
- risk_level
- requires_approval
- status
```

SQL 工具规则：

- 默认只允许参数化 `read` 查询；`write/ddl` 默认 `review` 或 `deny`。
- `sql_template` 使用命名参数，不拼接 agent 输出的原始 SQL。
- 每次执行记录 `query_hash`、参数 hash、行数、耗时、脱敏摘要。
- 授权资源命名为 `sql_connection:{id}`、`sql_tool:{id}`、`sql_query:{query_hash}`。
- Python SQL wrapper 调 Rust `/internal/tool-calls:authorize` 后，由 Rust SQL executor 或受控 SQL service 执行；不建议 Python 直接持有数据库密码。

### 7.3 Conversation/Run/Event

已有接口：

```http
POST /api/v1/conversations
GET  /api/v1/conversations
POST /api/v1/conversations/{conversation_id}/runs:stream
GET  /api/v1/conversations/{conversation_id}/events
GET  /api/v1/conversations/{conversation_id}/ws
GET  /api/v1/runs
GET  /api/v1/runs/{run_id}
POST /api/v1/runs/{run_id}/cancel
```

需要补齐：

```http
POST /api/v1/runs/{run_id}/resume-user-input
GET  /api/v1/runs/{run_id}/events
GET  /api/v1/runs/{run_id}/tasks
GET  /api/v1/runs/{run_id}/subagents
```

Run 状态机：

```text
queued -> running
running -> waiting_approval
running -> waiting_user_input
running -> cancelling
running -> completed
running -> failed
waiting_approval -> running
waiting_approval -> failed
waiting_user_input -> running
cancelling -> cancelled
queued|running|waiting_* -> expired
```

只有 Rust 可以更新 run 状态。Python 只能提交事件，由 Rust 根据事件类型和当前状态机转换。

### 7.4 Internal runtime API

Rust 暴露给 Python：

```http
POST /internal/run-events
POST /internal/tool-calls:authorize
POST /internal/authz/check
POST /internal/authz/batch-check
GET  /internal/runtime-credentials/{runtime_credential_id}?tenant_id=&run_id=
POST /internal/files/read
POST /internal/files/write
POST /internal/files/edit
GET  /internal/files/list
GET  /internal/files/glob
POST /internal/files/search
POST /internal/local-exec/requests
POST /internal/agent-runs/{run_id}/resume
```

Python 暴露给 Rust：

```http
GET  /health
POST /internal/agent-runs
POST /internal/agent-runs/{run_id}/resume
POST /internal/agent-runs/{run_id}/cancel
```

服务间认证 v1 使用 shared bearer token；v2 改为短期签名 service token 或 mTLS。这里的 service token 只用于 Rust 与 Python/local executor 等内部服务通信，不作为用户登录 token，也不恢复本地 JWT 兼容层。当前启动脚本约定 `APP_AGENT_RUNTIME__SHARED_TOKEN` 默认等于 `APP_INTERNAL__SHARED_TOKEN`，Agent API 使用 `BIBI_AGENT__INTERNAL_TOKEN`；三者必须一致，否则 Rust dispatch 到 Python `/internal/agent-runs` 会被 401 拒绝。

#### 7.4.1 工具结果结构化视图契约

`/internal/run-events` 接收的 `tool.call.completed` payload 应允许可选 `views` 字段：

```json
{
  "run_id": "uuid",
  "tool_call_id": "uuid",
  "tool_name": "query_sales",
  "status": "completed",
  "output_summary": "返回 238 行销售数据",
  "views": [
    {
      "kind": "table",
      "title": "销售明细",
      "columns": [
        { "key": "region", "label": "区域", "type": "string" },
        { "key": "amount", "label": "金额", "type": "currency" }
      ],
      "rows_preview": [{ "region": "华东", "amount": 1200.5 }],
      "data_ref": {
        "artifact_id": "uuid",
        "object_reference_id": "uuid",
        "content_type": "application/json",
        "content_hash": "sha256:...",
        "size_bytes": 128000
      }
    }
  ]
}
```

Rust 侧处理规则：

- `output_summary/error_summary` 仍写入 `tool_calls` 和审计 evidence；富结果不进入摘要字段。
- `views` 必须由 Rust 做 schema 白名单校验，v1 只接受 `table/chart/map/json/file_diff/markdown/artifact`。
- 单个事件内联 preview 必须限大小；超过阈值的数据必须先写入 RustFS/project artifact，并在事件中只保留 `ArtifactRef`。
- `data_ref/artifact_ref` 必须校验 tenant、run、content_hash、content_type、虚拟路径和对象可读性；校验通过后写入 `tool_result_artifacts`，并尽力绑定 `tool_call_id`；校验失败时丢弃该 view，但保留事件和摘要。
- 图表只允许声明式 `spec_kind='vega_lite'`；地图只允许 GeoJSON 数据引用和白名单 style/tile；HTML/JS/动态组件不属于工具结果协议。
- 已校验 `views` 保留在 `run_events.payload` 供历史回放；大对象元数据写入 `tool_result_artifacts`，前端通过 `/api/v1/tool-result-artifacts/read?tenant_id=...&object_reference_id=...&offset=...&limit=...` 按需读取。

Python 侧处理规则：

- `PlatformToolWrapper` 执行工具后，先应用 obligations 脱敏和输出大小限制，再调用 `ToolResultPresenter` 生成 `views`。
- Presenter 根据工具名、工具版本 `output_schema/ui_hints`、结果类型和 content type 选择 view，不生成 React/HTML/JS；当前会规范化 `ui_hints`、`x-ui-hints`、`renderer`、`data-grid/vega-lite/geojson` 等常见别名为 `table/chart/map/json`。
- Presenter 对未知或超大结构 fallback 到 `json` preview + artifact ref；不为了渲染失败阻断工具结果返回。
- 原始工具返回值仍返回给 deepagents 后续推理；发送给 Rust 的展示数据必须是治理后的副本。

### 7.5 文件与 RustFS

Rust File Service 对外只暴露虚拟路径：

```text
/workspace/...
/scratch/...
/memories/...
/artifacts/...
```

面向前端的 public file API 只开放 project-scoped 读取能力，actor 来自 FerrisKey middleware 注入的 `PlatformRequestContext`，不接受请求体伪造 `actor_user_id/roles/session/device`：

```http
GET  /api/v1/projects/{project_id}/files?tenant_id={tenant_id}&prefix=/workspace/&pattern=*.md
GET  /api/v1/projects/{project_id}/files/read?tenant_id={tenant_id}&path=/workspace/report.md
POST /api/v1/projects/{project_id}/files:search
GET  /api/v1/projects/{project_id}/files/history?tenant_id={tenant_id}&path=/workspace/report.md
GET  /api/v1/projects/{project_id}/artifacts?tenant_id={tenant_id}&run_id={run_id}
```

浏览器侧文件写入、编辑、lock acquire/release 暂不开放 public API，仍由 Python runtime/local executor 通过 internal file service 调用。

新增表建议：

```text
object_references
- id
- tenant_id
- bucket
- object_key
- version_id
- etag
- content_hash
- size_bytes
- content_type
- owner_resource_type
- owner_resource_id
- created_at

file_locks
- id
- tenant_id
- project_id
- path_hash
- holder_run_id
- holder_user_id
- expires_at
- created_at
```

写入流程：

```text
FileWriteRequest
  -> validate tenant/project/path
  -> authz file:{project_id}:{path_hash}#write
  -> read latest revision
  -> compare expected_revision
  -> write object to RustFS
  -> insert object_references
  -> insert file_revisions
  -> insert run_events(file.changed)
  -> insert audit_logs
```

路径防护必须借鉴 Open Cowork 的 containment 思路：

- 拒绝空路径、NUL byte、UNC 非允许路径、`..` 逃逸。
- 先规范化虚拟路径，再映射 backend mount。
- 本地路径必须 realpath 后验证在 workspace root 内。
- 写操作必须带 `expected_revision`。

### 7.6 记忆接口

用户级四层记忆：

```text
core_profile
episodic
semantic
procedural
```

公共接口：

```http
GET  /api/v1/memories
POST /api/v1/memories
PATCH /api/v1/memories/{memory_id}
POST /api/v1/memories/{memory_id}/activate
POST /api/v1/memories/{memory_id}/reject
POST /api/v1/memories/{memory_id}/archive
DELETE /api/v1/memories/{memory_id}
POST /api/v1/memories:search
POST /api/v1/memories:batch-decision
```

内部接口：

```http
POST /internal/memory/retrieve-for-run
POST /internal/memory/candidates
POST /internal/memory/access-log
```

规则：

- Agent 不能直接写长期记忆，只能提交 candidate。
- Rust memory service 检索、过滤、脱敏、标记为 untrusted context 后写入 run snapshot。
- `core_profile` 和 `procedural` 默认需要用户或管理员审核后激活。
- 所有注入和访问写 `memory_access_logs`。

### 7.7 工作流 DAG

工作流分三层：

```text
workflow_designs: 草稿设计，可不完整
workflow_versions: 已编译版本，运行只使用 immutable compiled_plan
workflow_runs / workflow_node_runs: 运行事实
```

#### 7.7.1 DAG 与 Agent 创建的关系

Agent 创建和 DAG 工作流是两个不同层次：

```text
Agent/AgentVersion:
  定义一个可被调用的专家能力，包括 prompt、LLM profile、skills、tools、MCP、subagents、权限和风险策略。

WorkflowDesign/WorkflowVersion:
  定义多个任务节点之间的依赖关系，每个可执行节点引用一个已发布 AgentVersion。

WorkflowRun/WorkflowNodeRun:
  是一次具体执行。每个 node run 在 ready 后创建一个普通 agent run，由 Python deepagents 执行。
```

也就是说：

- 管理员先创建 Agent，并发布 AgentVersion。
- Workflow 设计时只选择 AgentVersion，不直接编辑 Agent draft。
- WorkflowVersion 编译时固化 node -> AgentVersion 的引用、输入映射、输出映射和权限快照。
- WorkflowRun 执行时，每个可运行节点都会创建独立 `runs` 记录，复用同一套 Run Gateway、审批、事件、审计和文件/工具权限。
- Workflow 不绕过 Agent 的能力绑定。节点能用的 skill/tool/MCP 是该 AgentVersion 的能力集与当前 actor 权限交集。

推荐执行流程：

```text
1. alon 创建并发布 AgentVersion A: "教务专家"
2. alon 创建并发布 AgentVersion B: "文档审查专家"
3. alon 创建 WorkflowDesign
4. Workflow node-1 绑定 AgentVersion A
5. Workflow node-2 绑定 AgentVersion B，并依赖 node-1
6. 发布 WorkflowVersion，Rust 编译并校验所有节点的 AgentVersion、工具、Skill、MCP、项目权限
7. alice 运行 WorkflowVersion
8. Rust 创建 workflow_run 与 workflow_node_runs
9. scheduler 发现 node-1 ready，创建 conversation run / agent run
10. Python 执行 node-1，事件写回 Rust
11. node-1 completed 后，Rust 把输出 summary/artifacts/file changes 注入 node-2 input
12. scheduler 创建 node-2 的 agent run
13. 所有 terminal 节点完成后，workflow_run completed
```

Workflow 节点引用模型：

```json
{
  "node_key": "academic_review",
  "node_type": "agent_task",
  "agent_id": "uuid",
  "agent_version_id": "uuid",
  "title": "审核教务材料",
  "instruction": "根据输入材料生成审核意见",
  "expected_output_schema": {},
  "input_mapping": {
    "user_input": "$.workflow.input",
    "upstream_summary": "$.nodes.prepare_material.output.summary"
  },
  "output_mapping": {
    "summary": "$.agent.final_summary",
    "artifacts": "$.run.artifacts",
    "changed_files": "$.run.changed_files"
  },
  "retry_policy": {
    "max_attempts": 2,
    "backoff_sec": 30
  },
  "timeout_sec": 1800
}
```

编译期必须校验：

- `agent_version_id` 已发布且未禁用。
- 当前发布者有 `workflow:{id}#manage`。
- 运行者或目标角色有 `agent_version:{id}#run`。
- AgentVersion 绑定的 skill/tool/MCP 对运行者可见并可用。
- Workflow node 的项目、文件、SQL、MCP、本地执行资源都能在运行时通过二次授权。
- DAG 无环，所有引用的 upstream node 存在。

接口：

```http
POST /api/v1/workflow-designs
GET  /api/v1/workflow-designs
GET  /api/v1/workflow-designs/{workflow_design_id}
PATCH /api/v1/workflow-designs/{workflow_design_id}
GET  /api/v1/workflow-designs/{workflow_design_id}/versions
POST /api/v1/workflow-designs/{workflow_design_id}/versions
GET  /api/v1/workflow-versions/{workflow_version_id}
POST /api/v1/workflow-versions/{workflow_version_id}/validate
GET  /api/v1/workflow-runs
POST /api/v1/workflow-runs
GET  /api/v1/workflow-runs/{workflow_run_id}
GET  /api/v1/workflow-runs/{workflow_run_id}/node-runs
POST /api/v1/workflow-runs/{workflow_run_id}/cancel
POST /internal/workflow-runs/{workflow_run_id}/tick
```

编译规则借鉴 Ordinus：

- Node = task，不等于 agent。
- Edge = dependency，是依赖唯一事实源。
- v1 只支持 agent-task node，不支持循环、条件分支、函数节点。
- 编译时校验空节点、缺字段、缺 agent、缺权限、环、孤立无效边。
- 每个 node 生成一个 `workflow_node_run`，ready 后创建独立 agent `run`。
- 上游节点输出以结构化 summary/artifact refs 注入下游节点 run input。
- 每个 node run 创建 run 时都生成自己的 `run_config_snapshot`，其中包含该节点引用的 AgentVersion、LLM profile、skills、tools、MCP、file mounts、memory context 和 policy version。

节点状态：

```text
pending
ready
queued
running
waiting_approval
waiting_user_input
completed
failed
skipped
cancelled
blocked
```

## 8. Python 端模块设计

建议把 `bibi_work_agent/main.py` 拆为：

```text
bibi_work_agent/
  __init__.py
  main.py
  settings.py
  api/
    app.py
    internal_routes.py
    schemas.py
  clients/
    rust_client.py
  runtime/
    agent_factory.py
    run_executor.py
    resume_executor.py
    event_normalizer.py
    checkpointer.py
    snapshots.py
  backends/
    platform_composite_backend.py
    project_file_backend.py
    scratch_backend.py
    memory_backend.py
    policy_backend.py
  tools/
    wrapper.py
    mcp_wrapper.py
    file_tools.py
    local_exec_tool.py
    memory_tools.py
    risk.py
  workers/
    celery_app.py
    tasks.py
```

高内聚规则：

- `api` 只接收 Rust internal 请求。
- `clients` 只封装 Rust HTTP 调用和重试。
- `runtime` 只处理 deepagents 构建、stream、checkpoint、resume。
- `backends` 只实现文件/状态 backend。
- `tools` 只实现工具包装、风险分类和二次校验。
- `workers` 只处理 Celery task 生命周期。

## 9. Python runtime 契约

### 9.1 DispatchRunRequest

Rust 调 Python：

```json
{
  "tenant_id": "uuid",
  "conversation_id": "uuid",
  "run_id": "uuid",
  "trace_id": "string",
  "input": {
    "messages": [],
    "user_prompt": "string",
    "workflow_node": null
  },
  "run_config_snapshot": {
    "actor": {
      "user_id": "uuid",
      "device_id": "uuid|null",
      "session_id": "uuid|null",
      "roles": ["tenant_member"]
    },
    "agent": {
      "agent_id": "uuid",
      "agent_version_id": "uuid",
      "system_prompt": "string",
      "model_profile_id": "uuid",
      "model": {
        "provider": "openai-compatible",
        "model_name": "gpt-5.1",
        "base_url": "https://example-compatible-endpoint",
        "parameters": {
          "temperature": 0.2,
          "max_output_tokens": 4096
        },
        "credential": {
          "has_secret_ref": true,
          "runtime_credential_id": "short-lived-id"
        }
      }
    },
    "tools": [
      {
        "tool_version_id": "uuid",
        "name": "read_file",
        "schema_hash": "sha256",
        "risk_level": "low"
      }
    ],
    "skills": [
      {
        "skill_version_id": "uuid",
        "manifest_hash": "sha256",
        "content_ref": "rustfs://bibi-work-marketplace/..."
      }
    ],
    "mcp_tools": [
      {
        "mcp_tool_id": "uuid",
        "server_id": "uuid",
        "tool_name": "search_docs",
        "schema_hash": "sha256",
        "credential_ref": "vault://tenant/mcp/server"
      }
    ],
    "sql_tools": [
      {
        "sql_tool_version_id": "uuid",
        "connection_id": "uuid",
        "operation": "read",
        "query_hash": "sha256",
        "parameter_schema": {}
      }
    ],
    "subagents": [
      {
        "subagent_key": "reviewer",
        "agent_version_id": "uuid",
        "tool_scope": "explicit"
      }
    ],
    "permissions": [],
    "file_mounts": [],
    "memory_context": [],
    "policy_version": "local-policy-v1",
    "risk_policy_version": "local-risk-v1",
    "token_policy_version": "token-policy-v1",
    "credential_policy_version": "credential-policy-v1",
    "thread_id": "tenant:conversation:run"
  }
}
```

要求：

- Python 不从数据库补权限，不自行扩展工具列表。
- Python 只使用 snapshot 中的 agent/tool/skill/MCP/SQL/file/memory/model 配置。
- Python 不持久化 provider API key、MCP token、数据库密码；如确实需要调用外部 LLM，只接收 Rust 签发或解析后的短期 runtime credential。
- snapshot 是运行时可审计证据，创建 run 后不随 draft 变动。

### 9.2 deepagents 创建

Python 构建：

```python
agent = create_deep_agent(
    model=model,
    system_prompt=system_prompt,
    tools=platform_wrapped_tools,
    subagents=compiled_subagents,
    backend=platform_composite_backend,
    permissions=filesystem_permissions,
    interrupt_on=interrupt_config,
    checkpointer=durable_checkpointer,
)
```

backend 映射：

```text
/workspace/ -> PlatformProjectBackend -> Rust File Service
/scratch/   -> RunScopedScratchBackend -> RustFS bibi-work-runs 或状态 backend
/memories/  -> PlatformMemoryReadOnlyBackend -> Rust Memory Service
/policies/  -> PlatformPolicyReadOnlyBackend -> Rust policy snapshot
```

### 9.3 事件归一化

Python 应把 deepagents 原始 stream 转成平台事件：

```text
messages token/chunk -> message.delta
AIMessage completed -> message.completed
tool call chunk -> tool.call.delta
tool call start -> tool.call.started
ToolMessage -> tool.call.completed/tool.call.failed
todos state -> task.created/task.updated/task.completed
task tool call -> subagent.started
task ToolMessage -> subagent.completed
__interrupt__ -> interrupt.requested + approval.requested
files state -> file.changed
terminal -> run.completed/run.failed/run.cancelled
```

事件格式：

```json
{
  "event_id": "stable-id",
  "type": "tool.call.started",
  "payload": {
    "run_id": "uuid",
    "tool_call_id": "string",
    "tool_name": "read_file",
    "summary": "read /workspace/README.md"
  },
  "trace_id": "string"
}
```

完成事件可以带结构化视图，但不要求所有工具都生成：

```json
{
  "event_id": "tool.call.completed.<tool_call_id>",
  "type": "tool.call.completed",
  "payload": {
    "run_id": "uuid",
    "tool_call_id": "uuid",
    "tool_name": "query_sales",
    "status": "completed",
    "output_summary": "返回 238 行销售数据",
    "views": [
      {
        "kind": "chart",
        "title": "区域销售额",
        "spec_kind": "vega_lite",
        "spec": {
          "mark": "bar",
          "encoding": {
            "x": { "field": "region", "type": "nominal" },
            "y": { "field": "amount", "type": "quantitative" }
          }
        },
        "data_ref": {
          "artifact_id": "uuid",
          "content_type": "application/json",
          "content_hash": "sha256:...",
          "size_bytes": 128000
        }
      }
    ]
  },
  "trace_id": "string"
}
```

### 9.4 Tool wrapper

所有工具执行前：

```text
normalize name/resource
hash args
classify risk
POST /internal/tool-calls:authorize
decision allow -> execute
decision review -> emit approval/interrupt and pause
decision deny -> return denied tool message
```

所有工具执行后：

```text
execute tool
apply obligations: redact/truncate/output policy
derive output_summary/error_summary
ToolResultPresenter.build_views(governed_result, tool_schema, ui_hints)
large result -> RustFS artifact/object_reference
emit tool.call.completed/failed with summary + optional views
```

`ToolResultPresenter` v1 规则：

- list[dict] 或 dataframe-like 结果 -> `table`，只内联前 N 行，完整数据写 artifact。
- numeric grouped result 或工具声明 `ui_hints.chart` -> `chart`，只允许 Vega-Lite 子集。
- GeoJSON 或工具声明 `ui_hints.map` -> `map`，数据必须写 artifact。
- dict/list 未命中特定 renderer -> `json` preview。
- file write/edit 结果 -> `file_diff` 或 `artifact`，引用虚拟路径、revision、hash，不暴露本地真实路径。
- 任意字符串 -> 保持 `markdown` 或摘要，不把字符串当 HTML 执行。

风险基线：

| 类型 | 默认风险 | 处理 |
| --- | --- | --- |
| read/list/glob/grep project file | low | 二次鉴权，通常 allow |
| write/edit project file | medium | revision 检查，可按策略 review |
| MCP read-only | low/medium | 按 server/tool policy |
| MCP write/action | high | 默认 review |
| local command | high/critical | 默认 review + local exec policy |
| SQL read | medium | query hash/schema/table 鉴权 |
| SQL write/DDL | critical | 默认 deny 或强制审批 |
| memory candidate | low/medium | 只能提交 candidate |

### 9.5 Durable checkpointer

Openwork 的 sql.js checkpointer 适合桌面本地，但企业平台应使用 Postgres/Redis-backed checkpointer。v1 可先实现：

```text
agent_checkpoints
- tenant_id
- thread_id
- checkpoint_ns
- checkpoint_id
- parent_checkpoint_id
- type
- checkpoint_json
- metadata_json
- created_at

agent_checkpoint_writes
- tenant_id
- thread_id
- checkpoint_ns
- checkpoint_id
- task_id
- idx
- channel
- type
- value_json
```

恢复要求：

- `thread_id` 必须由 Rust 生成，绑定 `tenant_id/conversation_id/run_id`。
- 审批恢复必须使用同一个 `thread_id` 和最新 checkpoint。
- resume 要幂等，重复 approval decision 不得重复执行高风险工具。

## 10. 审批设计

审批由 Rust 管理，Python 只触发和恢复。

流程：

```text
Python wrapper -> /internal/tool-calls:authorize
  -> Rust decision=review
  -> Rust insert tool_calls/approvals/interrupts
  -> Rust update run waiting_approval
  -> Python emits interrupt.requested
  -> Python checkpoint and parks task
  -> Approver calls /api/v1/approvals/{id}/decision
  -> Rust authz approval:{id}#approve
  -> Rust update approval/run/event/audit
  -> Rust calls Python /internal/agent-runs/{run_id}/resume
  -> Python resumes deepagents checkpoint
```

审批幂等：

- `approvals.status` 必须从 `pending` 原子更新为 `approved/rejected/expired`。
- Python resume 使用 `approval_id` 作为 idempotency key。
- `tool_calls` 记录原始 args_hash 和 decision。
- 如果 run 已 terminal，审批 decision 返回 conflict。

## 11. 审计与事件

### 11.1 标准事件类型

```text
run.queued
run.started
message.delta
message.completed
tool.call.started
tool.call.delta
tool.call.completed
tool.call.failed
interrupt.requested
approval.requested
approval.completed
task.created
task.updated
task.completed
subagent.started
subagent.message.delta
subagent.tool.call.started
subagent.completed
file.changed
memory.candidate.created
memory.activated
workflow.node.queued
workflow.node.running
workflow.node.completed
workflow.node.failed
local_exec.started
local_exec.output
local_exec.completed
run.completed
run.failed
run.cancelled
```

### 11.2 Outbox 规则

写事件必须：

```text
BEGIN
insert run_events
insert event_outbox(target='redis_stream', payload=event)
insert audit_logs if needed
COMMIT
publisher XADD Redis
mark outbox published by outbox.id
```

修复建议：

- `mark_outbox_delivered` 参数改为 `outbox_id`。
- `event_outbox` 增加 `last_error`。
- publisher 定时后台任务化，不依赖手动调用 `/internal/outbox/publish`。

### 11.3 审计 hash chain

`audit_logs` 应增加链式 hash：

```text
row_hash = sha256(canonical_json(row_without_hash) + prev_hash)
prev_hash = tenant 最新 audit row_hash
```

当前已实现基础链式写入：

- 新增 `features/agent_platform/audit.rs`，封装审计行 canonical JSON、`row_hash` 计算、租户级 `pg_advisory_xact_lock` 和插入逻辑。
- `write_authz_audit_tx` 不再直接裸写 `audit_logs`，改为调用审计模块；public/internal authz check、batch-check 和业务侧 `require_ferriskey_allow` 产生的授权审计都会写入 `prev_hash/row_hash`。
- 新增 `20260621000002_audit_hash_chain.sql`，为 tenant hash chain 查询和 row hash 唯一性补索引。
- 历史 `row_hash IS NULL` 的审计行不参与新链；新链从每个 tenant 第一条已哈希审计行开始。后续如需全量追溯，应单独设计离线 backfill 与校验报告，不能静默伪造历史完整性。
- 新增 `GET /api/v1/audit/hash-chain:verify?tenant_id={tenant_id}&limit=1000`，由 `audit_admin`、`tenant_admin`、`security_admin`、`platform_admin` 或显式 policy binding 访问。接口会在服务端重算最近 N 条已哈希审计行，校验相邻 `prev_hash` 串接和每行 `row_hash`，返回 `valid`、检查范围、首尾 hash 和 `broken_at`。
- `ResourceAuthzService` 已把 `audit_admin` 纳入管理员角色集合，避免审计管理员无法执行审计校验。
- 新增 `audit_hash_chain_segments` 和 `POST /api/v1/audit/hash-chain:seal`。接口从上一个 segment 的 `last_row_hash` 继续沿 `prev_hash` 链收集下一批未 seal 审计行，检测无新行、断链或 fork 后 fail closed；成功后写入 segment manifest、manifest hash、首尾 audit id、首尾 hash 和封存人。
- RustFS 启用时，seal 会把证据对象写入 `object_store.audit_bucket` 下的 `tenants/{tenant_id}/audit/hash-chain/yyyy/mm/dd/{segment_id}.json`，并以 `owner_resource_type='audit_hash_chain_segment'` 写入 `object_references`。对象存储禁用时仍会保留数据库 segment manifest，便于测试和降级环境验证。
- 审批决策成功后会把 `request_payload`、`decision_payload`、run/tool/approver 关联信息归档为 `approval_evidence`，写入 `object_store.audit_bucket` 下的 `tenants/{tenant_id}/audit/approvals/yyyy/mm/dd/{approval_id}.json`，并把 `object_references.id` 回填到 `approvals.evidence_object_reference_id`。对象写成功但事务提交失败会尽力删除孤儿 audit object。
- 新增 `audit_hash_chain` 配置段和 `features/agent_platform/audit_sealing.rs`，应用启动时可按 `auto_seal_enabled` 拉起后台 worker。worker 每轮查询存在未封存 hash-chain 起点的租户，按 `worker_tenant_batch_size` 限流，并对每个租户调用同一个 `seal_audit_hash_chain` 核心函数，`segment_max_rows` 控制单段大小。自动封存没有人工操作人，因此 `audit_hash_chain_segments.sealed_by_user_id` 与 manifest 中的 `sealed_by_user_id` 允许为 `null`。

高风险证据写 RustFS：

```text
bibi-work-audit/tenants/bibi-work/audit/tool-calls/yyyy/mm/dd/{tool_call_id}/{archived_at_unix_nanos}.json
bibi-work-audit/tenants/bibi-work/audit/approvals/yyyy/mm/dd/{approval_id}.json
bibi-work-audit/tenants/bibi-work/audit/hash-chain/yyyy/mm/dd/{segment_id}.json
```

当前实现已在 `object_store` 中区分 `files_bucket` 与 `audit_bucket`：普通文件对象写 `bibi-work-files`，hash-chain segment、approval evidence 和 high/critical tool-call evidence 写 `bibi-work-audit`。tool-call evidence 只归档 `args_hash`、输入/输出/错误摘要、风险等级、决策、run/conversation/trace 关联和最终状态，不保存原始工具参数或完整输出。仍待补齐更完整的归档脱敏策略、分区归档策略和历史 row_hash 回填策略。

## 12. 参考项目能力迁移

### 12.1 Open Cowork

可迁移：

- workspace path containment、realpath、防 `..`、防 UNC/绝对路径逃逸。
- permission rules 的保守默认：未知/格式错误 -> ask/review。
- MCP server 生命周期、stdio/SSE/streamable-http 连接、工具发现、工具名规范化和 hash suffix。
- memory 的 core/experience 提取、progressive retrieval、source workspace 标注。
- 本地 sandbox、WSL/Lima/native executor 思路。

必须改造：

- 不能让 renderer 或桌面端成为权限事实源。
- 不能只靠本地 permission popup 做审批；审批必须写 Rust `approvals` 和 `audit_logs`。
- MCP secret 不进普通配置文件，改为 secret ref。
- local command 不由 Python 直接 spawn，改为 Rust local exec coordinator。

### 12.2 Ordinus

可迁移：

- workflow design -> compiled plan 的纯函数编译模型。
- Node = task、Edge = dependency、边是依赖唯一事实源。
- run gating：空节点、超上限、缺字段、缺 agent、环检测。
- provider runtime contract：executable + args，不传 raw shell；cancel/timeout；事件归一化。
- centralized observability：运行快照、事件、liveness health、脱敏诊断。

必须改造：

- SQLite/Electron IPC 改为 Postgres/Rust API。
- 单机 work request 改为多租户 workflow_run/workflow_node_run。
- provider CLI adapter 改为 Python deepagents runtime + Rust scheduler。

### 12.3 Openwork

可迁移：

- deepagents stream mode 同时处理 `messages` 和 `values`。
- todos/tasks、workspace files、subagents、interrupt 的事件解析模型。
- HITL resume 使用 checkpoint + Command/resume 的思路。
- workspace watcher 对文件变更事件的投射思路。

必须改造：

- Openwork 直接本地文件和 shell 的模式不能作为企业默认模式。
- sql.js checkpointer 改为 Postgres/Redis-backed checkpointer。
- interrupt 不能只是 UI 状态，必须落 Rust approval/interrupt 表。

## 13. 分阶段开发计划

### 阶段 0：基础身份、配置和授权收敛

目标：先把平台安全边界固定。

工作项：

1. 破坏性移除本地 JWT 作为身份事实源的中间件和签发流程。
2. FerrisKey OIDC/JWKS 接入 Rust auth middleware。
3. roles claim 解析并写入 request actor context。
4. 创建 `platform_users`、`resource_relations`、`resource_policy_bindings`、`authz_decisions`、`ferriskey_role_projection`。
5. 实现 `ResourceAuthzEngine` trait 与 `LocalResourceAuthzEngine`。
6. 改造 `FerrisKeyClient` 为 `ResourceAuthzService`，不再直接假设 FerrisKey 资源级 PDP。
7. 为 alon/alice 建立 tenant membership 和初始资源关系。
8. 创建 `llm_providers`、`llm_credentials`、`llm_model_profiles`，并让 AgentVersion 只能引用已发布 model profile。
9. 创建 `sql_connections`、`sql_tool_versions`，把 SQL 能力纳入 Tool/Policy 管理。

验证：

- 使用 alon token 可以创建 agent/skill/mcp/tool。
- 使用 alice token 无法管理 agent，但可运行被授权 agent。
- 使用本地旧 JWT 调用受保护 API 必须失败。
- 未配置 LLM model profile 的 AgentVersion 不允许发布。
- 未注册 SQL tool version 的任意 SQL 字符串不能被 agent 执行。
- 未配置或不可用授权服务时，执行型接口 fail closed。
- authz check/batch-check 写 `authz_decisions` 和 `audit_logs`。

### 阶段 1：Run Gateway、事件与 outbox

目标：形成可靠运行事件面。

工作项：

1. 修复 outbox publisher 按 `event_outbox.id` 标记发布。
2. 把 publisher 做成后台任务或受控 internal job。
3. SSE 支持 `Last-Event-ID` 和 `after_seq` 回放。
4. WebSocket 支持订阅鉴权、心跳、after_seq、session revoke 断开。
5. Run 状态更新集中到 `RunService`。

验证：

- Python 提交事件后 Postgres 有完整 `run_events`。
- Redis 可丢失重建，不影响 Postgres 回放。
- SSE 断线后能从指定 seq 继续。
- terminal event 正确更新 run status。

### 阶段 2：Python runtime 工程化与 deepagents 接入

目标：Python 从骨架变成受控运行态。

工作项：

1. 拆分 Python 模块。
2. 实现 RustClient、EventEmitter、AgentEventNormalizer。
3. 实现 PlatformCompositeBackend。
4. 实现 Postgres/Redis-backed checkpointer。
5. Celery task 增加 idempotency key、timeout、cancel check。
6. deepagents stream 归一化为平台事件。

验证：

- 一个普通 run 能输出 `run.started/message.delta/message.completed/run.completed`。
- todos 映射为 `task.*`。
- task tool/subagent 映射为 `subagent.*`。
- worker 重试不会重复插入同 event_id。

### 阶段 3：工具、MCP 与审批闭环

目标：所有副作用可控、可审计、可恢复。

工作项：

1. 实现 Python ToolWrapper。
2. 实现 MCP wrapper，不让 agent 直接持有 MCP secret。
3. 实现 risk classifier。
4. 完整审批 resume：approval -> Python resume -> checkpoint continuation。
5. 工具输出脱敏、截断、审计摘要。
6. 工具结果结构化视图：Python `ToolResultPresenter` 生成 `ToolResultView[]`，Rust ingest 校验 `views` schema、artifact 引用和大小限制。

验证：

- alice 调用未授权 tool 返回 deny ToolMessage。
- 高风险 tool 返回 approval requested，run 进入 `waiting_approval`。
- approver 批准后 run 从 checkpoint 恢复。
- 表格/图表/地图/JSON 工具结果在 run event 回放后仍能得到相同 `views` 投影。
- 未识别或校验失败的 view 不影响 `output_summary`、工具审计和 run terminal 状态。
- 重复批准不会重复执行工具。

### 阶段 4：RustFS 文件、对象和本地执行

目标：远程文件和本地执行统一进 Rust 安全边界。

工作项：

1. 实现 RustFS client、object_references。
2. 文件写入落 RustFS，`file_revisions` 保存 version_id/etag/hash。
3. 文件读取支持最新版、历史 `revision` 和已记录 `version_id` selector，并校验对象内容 hash。
4. 实现 file conflict、lock、large object、binary object。
5. 对象写入后数据库失败时清理孤儿对象，并补审计证据。
6. 定义 local executor WebSocket/device protocol。
7. 实现 local exec request 状态机、短期 token、stdout/stderr event。

验证：

- 文件写入必须带 `expected_revision`，冲突返回 409。
- 文件读取默认返回最新版；指定 `revision` 或已记录的 `version_id` 时返回对应历史修订。
- RustFS 对象读取后必须校验 `content_hash`，不一致时 fail closed。
- Python 只能访问虚拟路径，不能看到本地真实路径。
- local exec 未审批不可执行高风险命令。
- local exec 输出按 max bytes 截断并写审计。

### 阶段 5：四层记忆

目标：记忆可检索、可治理、可审计。

工作项：

1. 补齐 memory_embeddings、memory_ingestion_jobs、memory_feedback。
2. run completed 后生成 candidate memories。
3. 记忆检索注入 run_config_snapshot。
4. 用户可激活、拒绝、编辑、删除记忆。
5. 注入内容标记为 untrusted memory context。

验证：

- alice 的 private memory 不会被其他用户检索。
- candidate 不会自动进入 core_profile，除非策略允许或用户确认。
- 每次 memory read/write 都有 access log。
- prompt injection 风险内容不会提升为 system 指令。

### 阶段 6：DAG 工作流调度

目标：专家智能体工作流可编译、可运行、可观测。

工作项：

1. 实现 workflow compiled_plan schema。
2. 实现 workflow validator：无环、节点字段、agent/tool/skill 权限。
3. 实现 WorkflowScheduler ready queue。
4. 节点创建独立 run，依赖完成后推进下游。
5. 节点重试、取消、超时、失败传播。

验证：

- 一个 3 节点 DAG 按依赖顺序执行。
- 并行节点可同时 queued/running，但受并发限制。
- 上游失败时下游 blocked/skipped。
- workflow events 和 run events 可统一查询。

## 14. 测试策略

Rust：

- unit：authz policy、path validation、run status transition、workflow compile validation。
- integration：API + Postgres + Redis，覆盖 run event/outbox/authz/audit。
- contract：Python internal API request/response schema。
- security：越权 agent/tool/file/memory/workflow/local_exec deny/review。

Python：

- unit：event normalizer、risk classifier、tool wrapper、snapshot parser。
- integration：FastAPI internal routes + mocked RustClient。
- worker：Celery idempotency、retry、timeout、resume。
- deepagents：使用 fake model/fake backend 验证 stream 和 checkpoint。

端到端：

- alon 创建 agent/skill/tool/MCP -> 授权 alice 运行。
- alice run -> file read/write -> approval -> resume -> completed。
- alice memory candidate -> user activate -> 下一次 run 注入。
- workflow DAG -> 多节点 run -> 事件回放。

## 15. 近期实现优先级

当前从资深项目架构师、Rust/Python 工程和智能体运行设计角度判断，近期应优先执行以下阶段：

1. 阶段 3 收尾主线：approval resume continuation、MCP/local exec wrapper 和 SQL/第三方工具 wrapper。当前 Python `PlatformToolWrapper` completed/failed 事件已通过 `RustClient` + `httpx.MockTransport` 验证 HTTP request/serialization 契约；同一工具事件契约也已通过真实 Rust internal HTTP 服务级 E2E，Rust 会反写 `tool_calls`、写入工具执行审计 hash-chain 行，并对 high/critical completed/failed 工具事件归档 `tool_call_evidence`；review 已进入 LangGraph `interrupt(...)`，审批通过后 Python resume endpoint 会入队 Celery，worker 使用 `Command(resume=...)` 和同一 `thread_id` 续跑，并以 `approval_id` 做 Redis 幂等；真实 deepagents graph + HITL interrupt + approve resume 回归已验证高风险工具不会在首轮或重复 approval 下重复执行；MCP/local exec/SQL/第三方 wrapper 已走 Rust internal 边界；Rust MCP HTTP JSON-RPC adapter、MCP `tools/list` discover 基础控制面、注册 SQL read tool executor 和第三方 HTTP executor 已有回归覆盖；LLM env-backed runtime credential resolver 已有最小闭环。下一步重点是 Vault/KMS resolver、secret-backed MCP discover/execute、MCP 完整生命周期、写类 SQL 审批策略和更细资源策略。
2. 阶段 4 收尾：在 `/internal/files/write|edit` 已接入 RustFS object write 和 `object_references`、`/internal/files/read` 已支持历史 `revision/version_id` selector、对象读取 hash 校验、孤儿对象清理、File Service HTTP API 级 E2E、二进制/大对象、file lock 和 RustFS 审计证据后，已补 `file_search_documents` 全文索引和虚拟目录 `entries`。下一步继续补生产级归档策略、索引重建/回填 worker 和大对象内容抽取策略。
3. 阶段 5 语义检索收尾：基于已给定 embed endpoint 和 Qdrant 配置继续补前端治理台、更细管理员治理策略和真实 runtime 端到端候选回归。Rust 已有 `memory_embeddings`、`memory_ingestion_jobs`、基础 indexing worker、真实 Qdrant indexing E2E、内部 retrieve-for-run、内部 candidates/access-log、候选批处理治理、四层治理核心回归、Conversation Run Gateway 和 workflow node run snapshot 注入，`run.completed` candidate payload 已能自动沉淀为待审核 memory；Python runtime 已新增保守自动候选提取。
4. 阶段 6 深化：基础 input/output mapping 已新增受限 JSONPath selector 和节点输入/输出投影，普通 run 与 workflow run 创建路径已对 AgentVersion 绑定的 skill/tool/MCP 做逐项 `use` 授权，且已补 AgentVersion 能力绑定 loader 的真实 Postgres 回归；Python runtime 已补三节点 workflow payload DAG E2E，覆盖 `execute_run_payload`、event normalizer、EventEmitter 和上下游输出传递。下一步应补 create-run/workflow-run API 级能力授权回归、阻塞中 agent 调用的强取消语义、更复杂 mapping 表达式，以及 Rust scheduler -> Python runtime service 的跨进程 DAG E2E。

前几轮实际优先补阶段 2/3 的 Python 平台边界，而没有直接进入 RustFS/Qdrant。原因是工具输入/输出治理、工具事件回写和 workspace list/search 能在现有 Rust internal API 下闭环验证，并能降低后续 MCP、local exec、memory 注入的泄露风险。随后已推进阶段 5：先固定 Python runtime 侧 embed/Qdrant 协议、脱敏和 untrusted 注入语义，再补 Rust 侧索引元数据、内部 retrieve-for-run、向量优先/关键词回退、access log 和基础 indexing worker。

后续建议接下来的代码顺序：

1. 为 FerrisKey OIDC/JWKS middleware 增加集成测试：mock discovery/JWKS、roles claim、kid rotation、expired/nbf/audience/issuer/azp 失败路径。
2. 调整 FerrisKey client scope/protocol mapper，让已分配 roles 进入 `realm_access.roles`，或明确把 Rust/Postgres role projection 作为 v1 授权事实源。
3. 为 public authz check/batch-check、run snapshot actor 注入和工具二次鉴权补越权回归测试。
4. 基于已完成的真实 deepagents/HITL E2E，继续补真实模型服务和长链 checkpoint 压测，验证多工具、多轮审批和 worker 重启后的续跑一致性。
5. 在已完成 `tool_calls.status/output_summary/error_summary/completed_at` 反写、数据库级回归、`ingest_run_events` handler 回归、`PlatformToolWrapper` -> `RustClient` HTTP request/serialization 回归、真实 Rust internal HTTP 服务级工具事件 E2E、真实 deepagents/HITL 回归、MCP/local exec/SQL/第三方 wrapper、MCP discover 基础控制面、Rust 侧真执行器和 LLM env-backed runtime credential resolver 基础上，继续补 Vault/KMS resolver、secret-backed MCP discover/execute、MCP 完整生命周期、写类 SQL 审批策略和更细资源策略。
6. 在已接入 RustFS object write、`object_references`、历史 revision/version selector、对象 hash 校验、孤儿对象清理、HTTP API 级 File Service + Postgres + RustFS E2E、二进制/大对象、file lock、文件写入/锁审计证据、全文索引和虚拟目录对象的基础上，继续补索引重建/回填、归档策略和大对象内容抽取策略。
7. 基于已实现的 Python/Rust memory 检索、indexing worker、真实 Qdrant indexing E2E、Conversation Run Gateway、workflow node run 注入、completed candidate 边界、Python runtime 保守自动候选提取、候选批处理治理和四层治理核心回归，补更细治理策略和真实 runtime 端到端测试。
8. 在已补基础 workflow input/output mapping、AgentVersion 能力逐项授权、能力绑定 loader 真实 Postgres 回归和 Python runtime DAG payload E2E 的基础上，继续补能力授权真实 API 回归、更复杂 mapping 表达式、阻塞中 agent 调用强取消语义和 Rust scheduler -> Python runtime service 跨进程 DAG 回归。
9. 在已完成授权审计 hash chain 基础写入、verify API、按需 segment sealing、RustFS segment manifest 证据对象、自动 sealing worker、审批证据归档、tool-call 高风险证据对象和独立 audit bucket 配置后，继续补历史回填、分区归档和脱敏归档策略。

这个顺序优先固化安全、事件一致性和运行恢复，避免先堆工具和智能体配置后再补权限边界。

## 16. 当前执行记录

更新日期：2026-06-21

本章已从逐轮执行流水账压缩为当前状态记录。判断口径如下：项目架构只记录跨阶段闭环能力和剩余阻塞；Rust 侧以路由、状态机、迁移、后台 worker、权限审计和模块边界为准；Python 侧以 runtime、backend、tool wrapper、checkpointer、memory prompt 注入和取消传播为准；智能体设计侧以可信运行快照、工具审批、记忆治理和 workflow 可审计执行为准。

### 16.1 阶段最新状态

| 阶段 | 当前状态 | 最新代码实现状态 | 核心缺口 |
| --- | --- | --- | --- |
| 阶段 0：认证、用户投影与授权 | 主线完成 | Rust 已接入 FerrisKey OIDC/JWKS Bearer token 校验，删除旧本地 JWT 登录；`platform_users`、`devices`、`platform_sessions` 作为本地业务投影；`GET /api/v1/me` 已返回当前用户、tenant membership、roles、UI capabilities、device 和 session 摘要；session/device revoke 与 logout 路由已存在；`ResourceAuthzService` 仍作为本地资源授权事实源，并已收紧 `tenant_member` 默认 file/memory 权限，且补齐 `audit_admin` 管理员语义；public/internal authz 与业务侧 `require_ferriskey_allow` 的授权审计已接入 tenant-scoped hash chain；`GET /api/v1/audit/hash-chain:verify` 已支持重算校验最近 N 条已哈希审计；`POST /api/v1/audit/hash-chain:seal` 已支持按需 segment sealing，并在 RustFS 启用时写入 `audit_bucket` manifest 证据对象和 `object_references`；审批 evidence 和 high/critical tool-call evidence 均写入 `audit_bucket`，分别回填 `approvals.evidence_object_reference_id` 与 `tool_calls.evidence_object_reference_id`；应用启动已支持可配置自动 sealing worker；Agent/Skill/Tool/MCP/LLM catalog 已补基础 list/get/PATCH/disable、版本列表/读取/禁用、AgentVersion validate/effective-capabilities，LLM credential 已补 list/create/revoke；`POST /api/v1/mcp-servers/{mcp_server_id}/tools:discover` 已支持无密钥 HTTP/JSON-RPC MCP `tools/list` 发现并 upsert `mcp_tools`；`/api/v1/policy-bindings` 已补 list/create/disable。 | FerrisKey token roles claim 在历史 smoke test 中仍未完整进入 access token；前端/桌面端 OIDC 登录流未在本仓库实现；session/device revoke 后主动断开 SSE/WebSocket 仍未闭环；catalog 仍缺 credential rotate、model profile test、OpenAPI/TS schema、Vault/KMS resolver、secret-backed discover、差异审计和更细只读可见性过滤；审计链仍缺历史回填、分区归档和更完整脱敏归档策略。 |
| 阶段 1：事件、outbox 与实时订阅 | 主线具备，可靠性待压测 | `event_store.rs` 承载 run event、SSE replay、Redis stream/PubSub publish 和 outbox retry；应用启动会拉起后台 outbox publisher；`/events`、`/events/stream` 和 WebSocket 已支持 replay + live，live 以 Redis Pub/Sub 唤醒并用 Postgres `run_events.seq` 做权威 backfill；Python dispatch 失败会收敛为 `run.failed`。 | 仍缺 SSE/WS 真实端到端和断线恢复压测；outbox publisher 缺 shutdown handle、指标、告警和 backpressure 策略；revoked session/device 不会主动关闭已有连接。 |
| 阶段 2：运行快照与 Python runtime | 部分完成，可信快照与基础续跑主线已落地 | `run_snapshot.rs` 从 AgentVersion、LLM profile、tool/skill/MCP bindings 编译普通 run 与 workflow node run 的 runtime snapshot，并由 Rust 覆盖 actor/run/model/tools/skills/mcp_tools/memory 等关键字段；新增 `handlers/capability_authz.rs`，普通 run 和 workflow run 创建时会把可用 AgentVersion 绑定能力映射到稳定 `skill_id/tool_id/mcp_tool_id`，并逐项执行 `use` 授权；AgentVersion 绑定接口已校验同租户/状态可用，且 AgentVersion 被 run 使用后拒绝继续改绑定；能力绑定 loader 已有真实 Postgres 回归；LLM credential 不再把 `secret_ref` 原文放入 runtime snapshot，只暴露 `has_secret_ref` 和短期 `runtime_credential_id`；Rust 在普通 run、workflow node run 和 approval resume dispatch 前解析 env-backed LLM credential 并写入 Redis 短期凭证；Python runtime 已拆为 `api/clients/runtime/backends/tools/workers`，具备 RustClient、EventEmitter、event normalizer、PlatformCompositeBackend 和 Postgres-backed checkpointer，并会通过 internal runtime credential endpoint 按 tenant/run scope 取回短期 LLM secret；OpenAI-compatible 模型配置已通过 `langchain-openai` 构造 ChatOpenAI，`base_url` 原样透传；Celery worker 已显式注册 `execute_run/resume_run` 任务；`PlatformCheckpointer` 已适配 LangGraph `BaseCheckpointSaver` 基础方法；普通 run 和 resume 均会传 `configurable.thread_id`；Rust cancel 会通知 Python，Python 在 stream 事件边界停止。 | 阻塞中的模型/工具调用不能强杀；能力授权仍缺 create-run/workflow-run API 级回归和更细风险策略；runtime credential 仍只支持 env-backed LLM，Vault/KMS、MCP/SQL secret resolver 和完整轮换策略仍未完成；完整 memory/policy backend 仍未完成；checkpoint saver 仍缺高性能 blob/delta 裁剪和更复杂真实 deepagents 压测。 |
| 阶段 3：工具、审批与输出治理 | 主线接入，真实 HITL 续跑、基础真执行器和富结果协议主链路已覆盖 | Python `PlatformToolWrapper` 已接入 agent factory；内置 file read/write/list/search 通过 `PlatformCompositeBackend` 回调 Rust；新增 `PlatformToolAdapters` 统一承载 file/MCP/local exec/SQL/第三方工具适配器；工具执行前调用 `/internal/tool-calls:authorize`，输入摘要和输出摘要会脱敏/截断；allow 后发 `tool.call.completed`，异常发 `tool.call.failed`；`ToolResultPresenter` 已能为治理后的工具结果生成 table/json/markdown/chart/map `views`，并把大表格写成 JSONL artifact、GeoJSON 写成 artifact 后生成 `data_ref`，可按规范化 `ui_hints` 做基础 view kind 选择；Rust ingest 会过滤非法 `views`，对 artifact object reference 做 tenant/run/hash/content-type 校验，写入 `tool_result_artifacts` 绑定，并反写 `tool_calls.status/output_summary/error_summary/completed_at`、写入工具执行审计 hash-chain 行，在 high/critical completed/failed 事件上归档摘要化 `tool_call_evidence` 到 audit bucket；public artifact read API 已支持按 object reference 读取，并对 JSON 数组和 JSONL 分页；前端已支持 artifact 表格 500 行分页和轻量虚拟滚动；review 分支已改为 LangGraph `interrupt(...)`，非图上下文仍显式 `ToolRequiresApproval`；executor 看到 `interrupt.requested` 会停止并避免误发 `run.completed`；`/internal/agent-runs/{run_id}/resume` 已改为 Celery 入队，resume worker 使用 `Command(resume=...)` 续跑并用 `approval_id` 做 Redis 幂等，且 failed 状态可被后续 worker 原子接管重试；真实 deepagents graph + HITL interrupt + approve resume 回归已验证高风险工具首轮不执行、审批后只执行一次、重复 approval 不创建/执行 agent；local exec 只创建 Rust `/internal/local-exec/requests`；MCP/SQL/第三方工具只调用 Rust internal endpoint；Rust 新增 MCP HTTP JSON-RPC adapter、MCP `tools/list` discover 协议模块和 public discover/upsert 控制面、注册 SQL read tool executor 和第三方 HTTP executor，secret-backed executor/discover 未配置 resolver 时 fail closed；Rust approval callback 会从 `runs` 表读取 `input/run_config_snapshot/thread_id/checkpoint_id/trace_id` 并重新签发 LLM runtime credential 后下发给 Python；真实 Rust internal HTTP 服务级 E2E 已验证工具事件落库和审计，执行器 ignored E2E 已验证 SQL/MCP/第三方 HTTP 链路；MCP discover 单元和 Postgres ignored 回归已覆盖 schema/hash 规范化与 upsert 幂等。 | 富结果仍缺真实跨进程 run E2E 和 object-store range/streaming reader；Vault/KMS resolver、secret-backed MCP/SQL、MCP 完整生命周期、写类 SQL 审批策略和更细风险策略仍需深化；阻塞中的模型/工具调用强取消仍未完成。 |
| 阶段 4：文件、对象与 RustFS | 主线能力闭环，基础全文索引、目录语义和前端只读查询已落地 | internal file API、路径校验、`file_revisions` 读写、`file.changed` 事件已抽到 `file_store.rs`；新增 `rustfs.rs` 作为 RustFS/object_store 适配层；新增 `file_lock.rs` 承载短期排他锁 acquire/release/写入校验；`/internal/files/write|edit` 在对象存储启用时写入 `bibi-work-files`，并记录 `object_references`、`file_revisions.object_reference_id`、bucket/key、etag、version_id、content_hash、content_type、is_binary、is_large；`/internal/files/read` 默认读最新版，并支持指定历史 `revision` 或已记录 `version_id`，二进制对象默认只返回 metadata，显式 `allow_binary=true` 才返回 `content_base64`；读取对象内容会校验 sha256；对象写成功但数据库持久化失败会尽力删除孤儿对象；文本且非大对象写入会同步 `file_search_documents` tsvector 索引；`/internal/files/search` 和 public `POST /api/v1/projects/{project_id}/files:search` 基于最新 revision 索引检索；`/api/v1/projects/{project_id}/files|files/read|files/history|artifacts` 已为基础前端提供只读文件树、预览、历史和产物浏览；`/internal/files/list|glob|search` 与 public file list/search 返回虚拟目录/文件 `entries`，Python scratch backend 也返回同构目录对象；文件写入、锁 acquire/release 均写入 `audit_logs` hash-chain 执行证据；File Service HTTP API + Postgres + RustFS ignored E2E 已覆盖 internal bearer middleware、handler 授权、写入对象引用、`expected_revision` 409 冲突、latest/historical revision 读取、可用时的 `version_id` 读取、file lock 冲突/释放、二进制对象、大对象和审计证据；`/internal/files/glob` 已支持基础 `*`/`?`。 | public file API 当前只读，不提供浏览器直接写入/lock；仍缺索引重建/历史回填 worker、大对象内容抽取策略和生产级归档策略。本地 RustFS/object_store PUT 未返回 `version_id` 时，`version_id` selector 只能在已有 version id 的对象行上工作，运行环境需确认 S3 version id 返回链路。 |
| 阶段 5：四层记忆、向量检索与候选治理 | 部分闭环，是当前实现最完整的新增能力 | `memory_service.rs` 已实现 memory CRUD、四层 layer/status/visibility/sensitivity 校验、`core_profile/procedural` candidate-only、单条 activate/reject/archive、`memories:batch-decision`、关键词 search 和 access log；`memory_embeddings`、`memory_ingestion_jobs`、`memory_feedback` 迁移已存在；`memory_vector.rs`、`memory_context.rs`、`memory_ingestion.rs` 支持 embed/Qdrant、Postgres 事实源过滤、approved 非 secret indexing、candidate/rejected/archived/secret/deleted 删除或跳过；`/internal/memory/retrieve-for-run|candidates|access-log` 已存在；conversation run 与 workflow node run 共用 `memory_injection.rs` 注入 `untrusted` memory context；`run.completed` 可按 payload 去重生成 candidate；Python runtime 已新增 `MemoryCandidateCollector`，从结构化结果、`memory_candidates`/`candidate_memories`/`memory.candidates` 和明确 memory candidates 文本区块中保守提取候选，并跳过疑似密钥内容；已有真实 Qdrant indexing 和 archive/reject 删除回归。 | 前端治理台未实现；自动候选构造策略仍偏保守，尚缺真实 deepagents runtime E2E 与指标；管理员级治理策略仍较粗，例如不同角色对 `core_profile/procedural/tenant/public` 的审核权限差异。 |
| 阶段 6：Workflow DAG 调度 | 部分完成，控制面读取闭环、DB 回归和 Python runtime payload E2E 已覆盖核心状态机 | `workflow_plan.rs` 校验 node key、agent task、agent version、边、自环、环、retry、timeout、并发限制和 mapping selector；`workflow_compile.rs` 校验节点 AgentVersion 属于 tenant 且 published，并固化权限/能力摘要；新增 `workflow_mapping.rs` 支持受限 JSONPath selector、递归 input/output mapping、节点 `node_input` 构造和 terminal output 投影；`workflow_scheduler.rs` 支持 design/version/run/cancel/tick、DAG 展开、ready 节点 dispatch、retry/backoff/timeout、终态保护、并发限制、节点 memory 注入和 terminal event 推进，并在 workflow run 创建阶段逐项校验节点 AgentVersion 绑定的 skill/tool/MCP 使用权限；public API 已补 workflow design list/get/PATCH、version list/get/validate、run list/get detail 和 node-runs 读取；`capability_authz.rs` 已补真实 Postgres 回归验证 AgentVersion skill/tool/MCP 绑定加载、上下文保留和去重；workflow node run 已复用 `run_snapshot.rs` 编译可被 Python 消费的 model/tools/skills/mcp snapshot；Python `execute_run_payload` 已有三节点 DAG payload E2E，覆盖 thread_id、workflow metadata、上游输出传递、event normalizer 和 EventEmitter 回写。 | 阻塞中的 agent 调用强取消未完成；mapping 表达式仍是保守子集，尚不支持过滤/函数/跨节点高级聚合；能力授权仍缺 create-run/workflow-run API 级回归；Rust scheduler -> Python runtime service 跨进程 DAG E2E 仍缺。 |

### 16.2 核心已实现事项

- 认证与授权边界：FerrisKey OIDC/JWKS、业务用户/session/device 投影、撤销 fail closed、本地 `ResourceAuthzService` 和关键业务 handler 的 actor 归一化已接入。
- 授权审计完整性：新增 `audit.rs` 统一写、验证和 seal `audit_logs` hash chain；授权审计行写入前会锁定 tenant 级链、读取上一条已哈希审计行并生成 `prev_hash/row_hash`，避免同租户并发写入造成链分叉；public verify API 可重算最近 N 条审计并定位 `prev_hash_mismatch` 或 `row_hash_mismatch`；public seal API 可把未 seal 链段固化为 `audit_hash_chain_segments`，并在 RustFS 启用时写 `audit_bucket` manifest evidence object；审批决策会归档 `approval_evidence` 并回填 `approvals.evidence_object_reference_id`；high/critical tool-call completed/failed 事件会归档摘要化 `tool_call_evidence` 并回填 `tool_calls.evidence_object_reference_id`；`audit_sealing.rs` 提供自动 worker，按配置周期扫描租户并复用同一 seal 函数生成 segment。
- 控制面基础生命周期：Agent/Skill/Tool/MCP/LLM provider/model profile 已有 list/get/PATCH/disable；LLM credential 已有 list/create/revoke 且响应只返回脱敏状态；Agent/Skill/Tool version 已有 list/get/publish/disable；AgentVersion 已有 bindings、validate 和 effective-capabilities；MCP server 已支持无密钥 HTTP/JSON-RPC `tools/list` discover 并把工具 schema/hash upsert 到 `mcp_tools`，MCP tools 已有 list/get/update/disable；`resource_policy_bindings` 已有 public list/create/disable 管理入口，策略写入做 subject/effect/risk 白名单校验。
- 运行可信性：普通 run 和 workflow node run 均由 Rust 从数据库事实源编译 runtime snapshot，客户端传入的 actor、model、tools、skills、MCP 和 memory context 不再直接可信；普通 run 与 workflow run 创建路径会对 AgentVersion 绑定的可用 skill/tool/MCP 做逐项 `use` 授权并写授权审计；AgentVersion 绑定会校验同租户和资源状态，且已被 run 使用的 AgentVersion 不能再变更绑定；LLM credential 只向 runtime snapshot 暴露是否存在 secret 和短期 `runtime_credential_id`，不暴露 `secret_ref` 原文；Rust 在 run dispatch、workflow node dispatch 和 approval resume 前解析 env-backed LLM secret，并以 tenant/run scope 写入 10 分钟 Redis runtime credential。
- 控制面敏感信息收敛：MCP server 的 list/get/create 响应不再返回原始 `config` 或 `secret_ref`，只返回 `transport`、`has_config`、`has_secret_ref`。
- 事件可观测性：run events、outbox、Redis Pub/Sub 唤醒、Postgres replay/backfill、SSE/WebSocket live 订阅和 dispatch 失败收敛已形成主线。
- Python 平台边界：runtime 模块化、workspace/scratch 虚拟路径、Rust 文件回调、Postgres checkpointer、LangGraph saver 基础适配、`Command(resume=...)` 审批续跑、`approval_id` Redis 幂等和 failed 接管重试、真实 deepagents/HITL 审批续跑回归、工具授权包装、MCP/local exec/SQL/第三方工具受控 wrapper、LLM runtime credential internal 获取与模型 api_key 注入、openai/openai-compatible ChatOpenAI 构造、用户 `base_url` 原样透传、Celery runtime task 显式注册、I/O 脱敏截断、工具完成/失败事件、RustClient HTTP request/serialization 回归、真实 Rust internal HTTP 工具事件 E2E、Python runtime 三节点 DAG payload E2E 和取消标记已实现。
- 文件对象边界：Rust 新增 `ObjectStoreSettings`、`RustFsClient` 和 `file_lock.rs`；文件写入会生成不可变 project-scoped object key，写入 RustFS 后记录 `object_references` 并把 `file_revisions` 关联到对象引用；读取支持 latest/revision/version_id selector，对象内容回填时校验 sha256；文本/base64 写入、content_type、二进制对象默认 metadata-only 读取、显式 `allow_binary=true` 返回 `content_base64`、大对象标记、短期排他锁 acquire/release 和写入锁校验已实现；数据库持久化失败时会尽力删除已写入的孤儿对象；测试禁用对象存储时保留 inline fallback；文本且非大对象写入会同步 `file_search_documents` 全文索引，internal 与 public list/search 已返回虚拟目录对象；public project file read/list/search/history/artifacts 已支持基础前端只读文件浏览；File Service HTTP E2E 已覆盖 internal middleware、授权、RustFS 对象引用落库、409 revision conflict、历史读取、file lock、二进制/大对象和审计证据。
- 记忆闭环主链路：candidate -> approved/rejected/archived 治理、向量索引 worker、retrieve-for-run、access log、run snapshot 注入、Python runtime 保守自动候选提取、completed candidate 沉淀和 Qdrant E2E 已具备。
- Workflow 控制面：DAG 校验、编译、展开、tick、retry/backoff/timeout、取消、并发限制、节点运行快照、节点 memory 注入、节点 AgentVersion 能力逐项授权、基础 input/output mapping 和 Python runtime DAG payload 执行已实现；public API 已补 design list/get/PATCH、version list/get/validate、run list/detail 和 node-runs，具备基础前端管理与回放读取面；相关 Rust 单元/DB ignored 回归和 Python E2E 已覆盖核心路径。
- 工具执行边界：`tool_execution.rs` 提供 MCP HTTP JSON-RPC adapter、注册 SQL read tool executor 和第三方 HTTP executor；`mcp_discovery.rs` 提供 MCP `tools/list` 发现、schema 规范化和服务端 schema_hash 计算；未注册工具、任意 SQL 字符串、非 read SQL、secret-backed MCP/SQL/HTTP 工具在对应 resolver 未实现前 fail closed；Python 只传平台资源 id 和 arguments，不持有 MCP secret、数据库密码或第三方 endpoint/header secret。
- 模块治理：`handlers.rs` 已收敛为 facade；事件、文件、workflow plan/runtime/compile、run snapshot/lifecycle、memory vector/context/ingestion 和多个 handler 子域已拆出。

### 16.3 核心未完成事项

- RustFS 仍需生产级深化：写入主路径已迁移到 RustFS/object reference，latest/revision/version_id selector、对象 hash 校验、孤儿对象清理、File Store + Postgres + RustFS 历史 revision 回归、File Service HTTP API E2E、二进制/大对象、file lock、审计证据、基础全文索引和目录对象语义已落地，但索引重建/历史回填、大对象内容抽取和归档策略仍需补齐；本地 RustFS/object_store PUT 当前未返回 `version_id`，需要确认 S3 version id 返回链路后再把 `version_id` E2E 固化为必过测试。
- 控制面生命周期仍需产品化：Agent/Skill/Tool/MCP/LLM 的基础 PATCH/disable/version list/get 已补，LLM credential list/create/revoke 已接入；仍缺 credential rotate、model profile test、OpenAPI/TypeScript schema 生成、secret-backed discover、discover 差异审计、停用缺失 MCP tool、基于角色/资源关系的更细只读可见性过滤，以及更完整的变更审计。
- AgentVersion 不可变语义仍需继续收敛：当前已阻止跨租户/不可用能力绑定，并禁止已被 run 使用的 AgentVersion 再修改绑定；但完整的发布时能力 manifest 冻结、绑定 hash、能力差异审计和基于冻结 manifest 的运行加载仍需补齐。
- 工具与审批仍需生产级深化：approval continuation 已接入 LangGraph interrupt、resume 入队、`Command(resume=...)`、幂等 guard、failed 接管重试和真实 deepagents/HITL 回归；MCP/local exec/SQL/第三方 wrapper 已接入 Rust internal 边界，MCP/SQL/第三方基础真执行器和 MCP discover 基础控制面已落地；LLM env-backed runtime credential resolver 已接入审批续跑；`ToolResultView` 协议、Python presenter、artifact 写入、Rust schema/object reference 校验、`tool_result_artifacts` 绑定、artifact 内容读取 API 和前端 Vega/Map/Table renderer 已落地；仍缺真实 run 回放 E2E、Vault/KMS resolver、secret-backed MCP/SQL、MCP 完整生命周期、写类 SQL 审批策略和更细风险策略。
- 实时订阅仍需生产级可靠性：SSE/WS 断线恢复压测、publisher lifecycle/指标/告警/backpressure、revoked session/device 主动断开仍未完成。
- Workflow 仍缺真实运行闭环：能力授权 create-run/workflow-run API 级回归、阻塞调用强取消、更复杂 mapping 表达式和 Rust scheduler -> Python runtime service 跨进程 DAG E2E 尚未完成。
- Memory 已有主链路，但治理产品化不足：前端治理台、自动 candidate 构造策略仍偏保守、角色化审核策略和运行指标仍需补齐。
- 模块分层还未到目标态：`memory_service.rs`、`workflow_scheduler.rs`、`run_service.rs` 和 `support.rs` 仍偏大，后续应继续拆 repository、DTO mapper、状态机、authz/audit helper 和数据库回归测试模块。
- 高风险审计与密钥治理仍未完整闭环：授权审计 hash chain、基础 verify API、按需 segment sealing、自动 sealing worker、RustFS audit bucket segment evidence、审批 evidence、tool-call 高风险 evidence、工具执行审计和文件对象/锁审计证据已接入；MCP 控制面响应和 LLM runtime snapshot 已避免返回 `secret_ref` 原文，LLM env-backed 短期 credential 注入已接入；但仍缺历史 row_hash 回填策略、分区归档策略、更完整脱敏归档策略、Vault/KMS resolver、MCP/SQL secret resolver 和凭证轮换审计。

### 16.4 当前验证状态

最新代码中可直接核对的实现入口：

```text
bibi_work_backend/src/features/agent_platform/mod.rs
bibi_work_backend/src/features/agent_platform/audit.rs
bibi_work_backend/src/features/agent_platform/audit_sealing.rs
bibi_work_backend/src/features/agent_platform/handlers.rs
bibi_work_backend/src/features/agent_platform/handlers/audit_service.rs
bibi_work_backend/src/startup.rs
bibi_work_backend/configuration/base.yaml
bibi_work_backend/src/features/agent_platform/handlers/capability_authz.rs
bibi_work_backend/src/features/agent_platform/handlers/agent_catalog_service.rs
bibi_work_backend/src/features/agent_platform/handlers/approval_service.rs
bibi_work_backend/src/features/agent_platform/handlers/file_service.rs
bibi_work_backend/src/features/agent_platform/handlers/mcp_catalog_service.rs
bibi_work_backend/src/features/agent_platform/mcp_discovery.rs
bibi_work_backend/src/features/agent_platform/handlers/tool_execution_service.rs
bibi_work_backend/src/features/agent_platform/tool_execution.rs
bibi_work_backend/src/features/agent_platform/handlers/run_service.rs
bibi_work_backend/src/features/agent_platform/handlers/policy_binding_service.rs
bibi_work_backend/src/features/agent_platform/runtime.rs
bibi_work_backend/src/features/agent_platform/run_snapshot.rs
bibi_work_backend/src/features/agent_platform/workflow_mapping.rs
bibi_work_backend/src/features/agent_platform/file_lock.rs
bibi_work_backend/src/features/agent_platform/file_store.rs
bibi_work_backend/src/features/agent_platform/rustfs.rs
bibi_work_backend/src/features/agent_platform/memory_vector.rs
bibi_work_backend/src/features/agent_platform/memory_ingestion.rs
bibi_work_backend/src/features/agent_platform/handlers/memory_injection.rs
bibi_work_backend/src/features/agent_platform/handlers/workflow_scheduler.rs
bibi_work_backend/migrations/20260621000001_tool_call_error_summary.sql
bibi_work_backend/migrations/20260621000002_audit_hash_chain.sql
bibi_work_backend/migrations/20260621000003_audit_hash_chain_segments.sql
bibi_work_backend/migrations/20260621000004_file_lock_binary_audit.sql
bibi_work_backend/migrations/20260621000005_approval_evidence_audit_bucket.sql
bibi_work_backend/migrations/20260621000006_tool_call_evidence_audit_bucket.sql
bibi_work_backend/migrations/20260621000007_tool_execution_file_index.sql
bibi_work_agent/clients/rust_client.py
bibi_work_agent/api/schemas.py
bibi_work_agent/runtime/agent_factory.py
bibi_work_agent/runtime/checkpointer.py
bibi_work_agent/runtime/resume_executor.py
bibi_work_agent/runtime/resume_idempotency.py
bibi_work_agent/backends/platform_composite_backend.py
bibi_work_agent/tools/platform_adapters.py
bibi_work_agent/tools/wrapper.py
bibi_work_agent/api/internal_routes.py
bibi_work_agent/tests/test_agent_factory.py
bibi_work_agent/tests/test_tool_wrapper.py
bibi_work_agent/tests/test_internal_routes.py
bibi_work_agent/tests/test_resume_executor.py
bibi_work_agent/tests/test_runtime_dag_e2e.py
bibi_work_agent/tests/test_platform_backend.py
bibi_work_agent/tests/test_checkpointer.py
bibi_work_agent/tests/test_cancellation.py
bibi_work_agent/tests/test_deepagents_hitl.py
bibi_work_agent/runtime/memory_retrieval.py
bibi_work_agent/runtime/memory_candidates.py
bibi_work_agent/runtime/run_executor.py
```

当前验证命令：

```text
cargo fmt
cargo fmt -- --check
cargo check
cargo test
cargo test secret_resolver
cargo test mcp_discovery
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work cargo test discovered_mcp_tools_are_upserted_with_schema_hash -- --ignored --nocapture
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work cargo test registered_ -- --ignored --test-threads=1
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work cargo test writes_and_reads_historical_inline_revisions -- --ignored --test-threads=1
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work REDIS_URL=redis://127.0.0.1:6380 RUSTFS_ENDPOINT=http://localhost:9004 RUSTFS_ACCESS_KEY=rustfsadmin RUSTFS_SECRET_KEY=rustfsadmin RUSTFS_REGION=us-east-1 RUSTFS_FILES_BUCKET=bibi-work-files RUSTFS_AUDIT_BUCKET=bibi-work-audit MEMORY_EMBEDDING_ENDPOINT=http://172.24.250.231:8335/embed QDRANT_REST_URL=http://127.0.0.1:6337 cargo test -- --ignored --nocapture
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work cargo test worker_seals_pending_audit_segments_without_actor_user -- --ignored
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work cargo test loads_agent_version_capability_requirements_from_postgres -- --ignored
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work sqlx migrate run
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work cargo test insert_audit_log_tx_chains_rows_per_tenant -- --ignored
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work RUSTFS_ENDPOINT=http://localhost:9004 RUSTFS_ACCESS_KEY=rustfsadmin RUSTFS_SECRET_KEY=rustfsadmin RUSTFS_REGION=us-east-1 RUSTFS_FILES_BUCKET=bibi-work-files RUSTFS_AUDIT_BUCKET=bibi-work-audit cargo test seal_audit_hash_chain_writes_manifest_object_to_rustfs -- --ignored
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work RUSTFS_ENDPOINT=http://localhost:9004 RUSTFS_ACCESS_KEY=rustfsadmin RUSTFS_SECRET_KEY=rustfsadmin RUSTFS_REGION=us-east-1 RUSTFS_FILES_BUCKET=bibi-work-files RUSTFS_AUDIT_BUCKET=bibi-work-audit cargo test archive_approval_evidence_writes_audit_bucket_object -- --ignored
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work RUSTFS_ENDPOINT=http://localhost:9004 RUSTFS_ACCESS_KEY=rustfsadmin RUSTFS_SECRET_KEY=rustfsadmin RUSTFS_REGION=us-east-1 RUSTFS_FILES_BUCKET=bibi-work-files RUSTFS_AUDIT_BUCKET=bibi-work-audit cargo test archive_tool_call_evidence_writes_audit_bucket_object -- --ignored
cargo test agent_platform::authz::tests
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work REDIS_URL=redis://127.0.0.1:6380 cargo test writes_and_reads_historical_inline_revisions -- --ignored
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work REDIS_URL=redis://127.0.0.1:6380 RUSTFS_ENDPOINT=http://localhost:9004 RUSTFS_ACCESS_KEY=rustfsadmin RUSTFS_SECRET_KEY=rustfsadmin RUSTFS_REGION=us-east-1 RUSTFS_FILES_BUCKET=bibi-work-files cargo test writes_and_reads_rustfs_historical_revision -- --ignored
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work REDIS_URL=redis://127.0.0.1:6380 RUSTFS_ENDPOINT=http://localhost:9004 RUSTFS_ACCESS_KEY=rustfsadmin RUSTFS_SECRET_KEY=rustfsadmin RUSTFS_REGION=us-east-1 RUSTFS_FILES_BUCKET=bibi-work-files cargo test file_service_http_round_trips_rustfs_revisions_and_conflicts -- --ignored --nocapture
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work REDIS_URL=redis://127.0.0.1:6380 cargo test tool_event_round_trips_through_internal_http_service -- --ignored --nocapture
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work REDIS_URL=redis://127.0.0.1:6380 cargo test failed_tool_event_persists_error_summary_without_overwriting_output -- --ignored --nocapture
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work REDIS_URL=redis://127.0.0.1:6380 cargo test ingest_run_events_handler_persists_failed_tool_event -- --ignored --nocapture
RUSTFS_ENDPOINT=http://localhost:9004 RUSTFS_ACCESS_KEY=rustfsadmin RUSTFS_SECRET_KEY=rustfsadmin RUSTFS_REGION=us-east-1 RUSTFS_FILES_BUCKET=bibi-work-files RUSTFS_AUDIT_BUCKET=bibi-work-audit cargo test local_rustfs_puts_and_reads_file_object -- --ignored
uv run pytest
uv run ruff check .
uv run pytest tests/test_agent_factory.py
uv run ruff check bibi_work_agent/clients/rust_client.py bibi_work_agent/runtime/agent_factory.py tests/test_agent_factory.py
uv run pytest tests/test_runtime_dag_e2e.py
uv run pytest tests/test_deepagents_hitl.py
uv run pytest tests/test_tool_wrapper.py tests/test_rust_client.py
uv run pytest tests/test_internal_routes.py tests/test_resume_executor.py tests/test_cancellation.py
uv run pytest tests/test_internal_routes.py tests/test_resume_executor.py tests/test_cancellation.py tests/test_tool_wrapper.py tests/test_checkpointer.py
rg -n "^### 16\." docs/enterprise-agent-platform-backend-development-plan.md
```

验证结果：

```text
2026-06-22 本轮 LLM Provider contract、Agent runtime token、Celery task 注册和 openai-compatible 真实运行链路验证通过：cargo check；cargo test resource_response_serializes_timestamps_as_rfc3339_strings；npm run test -- llm.adapter.test.ts；UV_CACHE_DIR=/tmp/uv-cache uv run --project bibi_work_agent pytest bibi_work_agent/tests/test_agent_factory.py bibi_work_agent/tests/test_celery_app.py。真实 smoke run 使用 env-backed credential 获取短期 runtime credential，Python worker 请求用户配置的 base_url 原样派生的 chat completions endpoint 并返回 200，run 状态 completed。
2026-06-22 本轮 LLM Provider/Profile/Credential 与 env-backed runtime credential resolver 验证通过：cargo check；cargo test secret_resolver；uv run pytest tests/test_agent_factory.py；uv run ruff check bibi_work_agent/clients/rust_client.py bibi_work_agent/runtime/agent_factory.py tests/test_agent_factory.py。
2026-06-21 本轮前端支撑接口补齐实际执行通过：cargo fmt；cargo check；cargo test（79 passed, 26 ignored）。本轮未改 Python runtime，未重新运行 `uv run pytest`；未运行需要 Postgres/RustFS/Qdrant/embed endpoint 的 ignored E2E。
本轮新增并验证通过的后端读取面：GET /api/v1/me；workflow design/version/run/node-runs list/get/detail/validate；public project file list/read/search/history/artifacts；catalog PATCH/disable/version list/get/AgentVersion validate/effective-capabilities。
Rust cargo check 通过：dev profile finished
Rust cargo test 通过：79 passed, 26 ignored
以下为历史验证记录：
MCP discover 单元测试通过：3 passed，覆盖 `tools/list` 响应解析、错误/非法 schema 拒绝和本地 HTTP JSON-RPC discover 请求。
MCP discover Postgres ignored 回归通过：1 passed，验证发现到的工具按 `(mcp_server_id, name)` 幂等 upsert，schema/hash 更新后不重复创建工具行。
Rust ignored E2E 本轮定向通过：2 passed，覆盖注册 SQL read tool、MCP HTTP JSON-RPC adapter 和第三方 HTTP executor；文件 inline revision ignored 回归 1 passed，覆盖 latest/revision 历史读取和 `file_search_documents` 最新 revision 索引检索。
上一轮 Python uv run pytest 通过：46 passed，1 warning（FastAPI/Starlette TestClient 依赖提示）
上一轮 Python uv run ruff check . 通过：All checks passed
上一轮 Python runtime DAG E2E 通过：1 passed，覆盖三节点 workflow payload 顺序执行、thread_id/workflow metadata、上游输出传递、event normalizer 和 EventEmitter 回写
上一轮 Python 真实 deepagents/HITL 回归通过：1 passed，覆盖真实 deepagents graph 首轮 high-risk tool interrupt 不执行、approve resume 后执行一次、重复 approval 被 `approval_id` 幂等挡住
上一轮 Python 阶段 3 聚合定向回归通过：18 passed，覆盖真实 deepagents/HITL、wrapper adapter、resume executor、ToolWrapper HTTP/脱敏/失败事件
上一轮 Python 定向审批续跑回归通过：3 passed，覆盖 resume worker 使用 `Command(resume=...)` 续跑、重复 `approval_id` 幂等跳过、高风险工具审批后不重复执行
审批 resume 回归通过：Python `/internal/agent-runs/{run_id}/resume` 已从 501 改为 Celery 入队；resume worker 会发 `run.started` 并以 `Command(resume=...)` 继续同一 `thread_id`；重复 approval 不创建 agent；Rust approval callback 会下发 `input/run_config_snapshot/thread_id/checkpoint_id/trace_id`；Rust runtime base_url 未配置时仍 resume fail closed
运行快照密钥脱敏单元测试通过：LLM credential runtime JSON 只包含 `has_secret_ref` 和 `runtime_credential_id` 占位，不包含 `secret_ref` 原文
审计 hash chain 单元测试通过：2 passed，验证相同行稳定 hash、不同 prev_hash 影响当前 row_hash
审计 hash chain Postgres ignored 回归通过：1 passed，验证同 tenant 连续两条审计行的 `prev_hash` 串接前一条 `row_hash`、verify 检测篡改、按需 seal 生成 segment、重复 seal 无新行时失败
审计 hash chain RustFS audit bucket evidence ignored 回归通过：1 passed，验证 seal 后 RustFS manifest evidence object 可从 `bibi-work-audit` 读取，且 `object_references.owner_resource_type='audit_hash_chain_segment'`
审批证据 RustFS audit bucket ignored 回归通过：1 passed，验证 `approval_evidence` 对象写入 `bibi-work-audit`，且 `object_references.owner_resource_type='approval_evidence'`、`approvals.evidence_object_reference_id` 正确回填
工具调用证据 RustFS audit bucket ignored 回归通过：1 passed，验证 `tool_call_evidence` 对象写入 `bibi-work-audit`，且 `object_references.owner_resource_type='tool_call_evidence'`、`tool_calls.evidence_object_reference_id` 正确回填
审计 hash chain 自动 sealing worker Postgres ignored 回归通过：1 passed，验证 worker 扫描未封存租户、按 `segment_max_rows` 连续生成两个 segment、自动封存时 `sealed_by_user_id` 和 manifest `sealed_by_user_id` 为 null、封存完成后不再重复扫描
Authz 单元测试通过：3 passed，验证 `audit_admin` 被视为管理员角色且 tenant_member 默认权限约束仍有效
PlatformToolWrapper HTTP 回归通过：2 个新增测试验证 completed/failed 工具事件通过 `RustClient` 真实序列化为 `/internal/tool-calls:authorize` 与 `/internal/run-events` 请求，且 Authorization header、trace_id、tool_call_id、输出/错误脱敏符合契约；定向 `tests/test_tool_wrapper.py tests/test_rust_client.py` 通过：10 passed
工具事件真实 Rust internal HTTP 服务级 E2E 通过：1 passed，验证 internal bearer middleware、`/tool-calls:authorize` 授权创建 `tool_calls`、`/run-events` ingest 反写 completed 状态和 output_summary、`run_events` 落库、`audit_logs.action='tool.call.completed'` 且 row_hash 非空
工具失败事件 Postgres ignored 回归通过：2 passed，验证 failed 事件写入 error_summary、不会覆盖已有 output_summary，并通过 handler ingest 路径生成 run_event/outbox
本地 Postgres migration 已执行：20260621000006/tool call evidence audit bucket 已处于最新
File Store + Postgres inline fallback ignored 回归通过：1 passed，验证 latest/revision 历史读取
File Store + Postgres + RustFS ignored 回归通过：1 passed，验证对象写入、对象内容回填、hash 校验和历史 revision 读取
File Service HTTP + Postgres + RustFS ignored E2E 通过：1 passed，验证 internal bearer middleware、handler 授权、RustFS 对象引用落库、`expected_revision` stale write 返回 409、latest/revision 历史读取、RustFS 返回 `version_id` 时按版本读取、file lock acquire/release 与锁冲突、二进制 `content_base64` 写入和显式 `allow_binary` 读取、大对象 `is_large` 标记、文件写入和锁操作审计证据
RustFS client ignored E2E 通过：1 passed，验证 bibi-work-files bucket put/get/get_opts/delete
第 16 章仍仅保留 16.1、16.2、16.3、16.4 四个小节
Workflow mapping 单元测试通过：4 passed，验证递归 input_mapping、terminal output_mapping、数组索引 selector、缺失路径返回 null 和非法 selector 拒绝；workflow plan mapping selector 校验通过：1 passed
Capability authz 单元测试通过：1 passed，验证 AgentVersion 能力授权需求按 resource/action 去重并保留工具上下文
Capability authz Postgres ignored 回归通过：1 passed，验证真实 catalog/binding 表中 AgentVersion skill/tool/MCP 绑定能加载为 use 授权需求、保留 tool_id/mcp_server_id 上下文并对同一 skill 多版本绑定去重
```

File Service + Postgres + RustFS 已补真正 HTTP API 级 E2E，且覆盖二进制/大对象、file lock 和审计证据；本轮新增基础全文索引和虚拟目录对象，Postgres inline ignored 回归已覆盖最新 revision 索引检索。工具事件已从 Python wrapper -> RustClient HTTP request/serialization 层推进到真实 Rust internal HTTP 服务级 E2E；审批 resume 已推进到真实 deepagents HITL、Celery resume 入队、`Command(resume=...)` 续跑、`approval_id` 幂等防重和 failed 接管重试。MCP/local exec/SQL/第三方 wrapper 已通过 Rust internal 边界接入，local exec 只排队请求，MCP/SQL/第三方基础真执行器已落地且 secret-backed executor fail closed；本轮补齐 MCP `tools/list` discover 基础控制面和 schema/hash upsert 回归；LLM env-backed runtime credential resolver 已覆盖普通 run、workflow node run 和 approval resume，并由 Python runtime 通过 internal endpoint 注入模型 api_key；审批 evidence、tool-call high/critical evidence 和 hash-chain evidence 已写入独立 `bibi-work-audit` bucket。Python runtime 已补三节点 DAG payload E2E。本地 RustFS/object_store PUT 当前未返回 `version_id` 时，`version_id` selector 仍只能在已有 version id 的对象行上工作。下一轮应优先补真实跨进程 run E2E 和 object-store range/streaming reader；随后再推进 Vault/KMS resolver、secret-backed MCP discover/execute、MCP server/tool 完整生命周期、写类 SQL 审批策略、索引重建/历史回填、阻塞调用强取消和 Rust scheduler -> Python runtime service 跨进程 DAG E2E。
