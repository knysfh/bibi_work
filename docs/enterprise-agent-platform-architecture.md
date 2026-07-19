# 企业级智能体运行平台架构

更新日期：2026-07-14

本文档只记录长期稳定的架构原则和系统边界。仓库中的文档职责如下：

- `enterprise-agent-platform-architecture.md`：架构决策与安全边界。
- `biwork-enterprise-agent-platform-execution-plan.md`：迁移步骤、完成状态与剩余工作。
- `biwork-api-contract.md`：renderer、Rust、Python 和桌面本地能力之间的接口合同。
- `local-service-config.md`：本地配置结构，不保存真实值。
- `local-testing-guide.md`：本地冒烟、E2E 与回归测试入口。

实现细节应进入代码、测试和上述对应主文档，不再为单个阶段、专项审计或局部 UI 策略新增平行文档。

## 1. 评估结论

本方案能够支撑目标系统的核心轮廓：Rust 后端作为唯一可信入口，FerrisKey 作为身份认证、realm、client、user、role、scope 和 token 中角色声明的事实源，Rust 后端作为资源级授权 PEP/PDP 适配层，Python deepagents 作为受控运行态，Redis Stream 作为实时流通道，Postgres 作为业务、事件和审计事实源，RustFS 负责远程项目文件、市场资源、运行产物、记忆文件和审计证据，Electron + React 负责桌面端与本地能力入口。

但它目前仍偏“原则性架构说明”，距离企业级智能体运行平台的生产级设计还缺少几个关键闭环：

1. 缺少明确的多租户、设备、会话、服务间认证和令牌撤销模型。
2. 缺少 FerrisKey OIDC/RBAC 与 Rust 后端资源级授权适配层之间的边界说明，包括角色声明、资源关系、策略版本、失败模式、缓存边界和批量校验规则。
3. 缺少运行态状态机、Celery 幂等、LangGraph/deepagents checkpointer、审批恢复和取消语义。
4. 缺少 Postgres 与 Redis 之间的事务 outbox，当前“先写库再投递流”的一致性边界还不够严格。
5. 缺少 DeepAgents 的 backend、permissions、HITL、subagents、streaming 与平台权限边界之间的映射方案。
6. 缺少远程 RustFS 与本地 local executor 的统一文件抽象、版本冲突、路径逃逸防护和本地执行控制协议。
7. 缺少用户四层记忆的治理模型，包括隐私、保留策略、记忆污染防护、人工编辑和评估闭环。
8. 缺少 MCP、Skill、Agent 配置的版本化、密钥管理、运行时可用性快照和审计要求。
9. 缺少工作流 DAG 的编译、调度、节点重试、并发、取消、人工输入和可观测性细节。
10. 缺少企业级可观测性、审计不可篡改、容量规划、灾备和测试策略。

因此，结论是：当前设计在方向上符合最佳实践，但还不能直接作为生产级详细设计。v1 方案应把平台拆成控制面、运行面、事件面、文件面、本地执行面和治理面，并把 FerrisKey 定位为身份与角色事实源，把 Rust 后端定位为企业资源级授权执行点，把 DeepAgents 当作受控 agent harness，而不是平台安全边界。

### 1.1 本地联调边界

本地联调依赖 FerrisKey、Postgres、Redis、RustFS、Rust 后端、Python Agent API/Celery 和 Electron。架构文档不记录本机端口、账号、密码、密钥或初始化状态；人工核对真实值读取 `docs/local-service-config.local.md`，测试进程只读取各服务的 `.env` 与 `.env.local`。具体启动和验证流程见 `docs/local-testing-guide.md`。

## 2. 需求支撑度判断

| 需求 | 当前文档支撑度 | 需要补强 |
| --- | --- | --- |
| Rust 后端统一入口 | 高 | 增加服务间认证、租户隔离、API 边界、失败模式 |
| FerrisKey OIDC/RBAC + Rust 资源级授权 | 中高 | 增加资源关系图、策略版本、缓存、批量校验、deny/review obligations |
| SSE 当前端流式输出 | 中 | 增加 Last-Event-ID、回放、心跳、背压、断线恢复 |
| WebSocket 多端订阅 | 中 | 增加订阅授权、after_seq、连接撤销、设备级 session |
| RustFS 远程文件管理 | 中 | 增加文件 revision、etag、锁/冲突、内容扫描、路径映射 |
| Python deepagents 运行态 | 中 | 增加 custom backend、HITL resume、checkpointer、event projection、Celery 幂等 |
| Celery 后台运行 | 低中 | 增加任务状态机、重试语义、重复事件去重、worker 隔离 |
| Redis 写入流式输出 | 中 | 增加事务 outbox、seq 分配、Redis 非事实源约束 |
| Electron + React 桌面端 | 中 | 增加本地 executor 协议、设备绑定、最小权限和安全升级 |
| 多端登录 | 低中 | 调整 `user_token` 唯一键，增加 devices/sessions/session_jti |
| Agent/Tool/Skill/MCP 管理 | 中 | 增加版本化、密钥 vault、配置快照、灰度/禁用 |
| 二次工具授权 | 中高 | 增加所有工具统一 wrapper、MCP wrapper、local exec wrapper、失败关闭 |
| 四层用户记忆 | 中 | 增加治理、检索注入、用户编辑、删除、评估、防 prompt injection |
| 审计日志 | 中高 | 增加不可篡改、脱敏、保留策略、审计查询权限、链式 hash |
| DAG 工作流可视化 | 中 | 增加编译模型、调度器状态机、节点重试、人工输入、并发控制 |
| task/subagent 显示 | 中 | 增加标准事件类型和 DeepAgents event streaming 映射 |

## 3. v1 架构原则

1. Rust 后端是唯一可信控制面和权限执行点。桌面端、浏览器端、Python 服务、local executor 都不能绕过 Rust 访问平台资源。
2. FerrisKey 是身份认证、realm/client/user/role/scope/token 的事实源。当前阶段 token 中的 roles 是粗粒度授权输入，资源级授权由 Rust 后端基于 FerrisKey roles、Postgres 资源关系和策略版本执行。
3. Python deepagents 只执行被 Rust 下发的 run，不保存平台权限，不直接暴露给用户端，不直接调用 local executor。
4. 所有可产生副作用的工具调用都必须走统一 tool wrapper，并在执行前进行 Rust 资源级二次校验；校验输入必须包含 FerrisKey token roles、资源关系和风险上下文。
5. DeepAgents `permissions`、HITL、backend policy hooks 是运行态防护层，不是企业权限边界。企业权限边界由 FerrisKey 身份与角色、Rust 资源级授权、Postgres 资源事实和审计闭环共同构成。
6. Postgres 是业务、运行事件、审批和审计事实源。Redis Stream 只做实时投递和短期断点续传。
7. 流事件必须先进入 Postgres append-only `run_events`，再通过 outbox 投递到 Redis 和 WebSocket/SSE。
8. 本地执行是最高风险面，必须通过设备绑定、短期执行令牌、路径白名单、命令审批、超时、输出限流和完整审计控制。
9. 记忆是非可信检索上下文。任何 memory 内容都不能提升为 system/developer 指令，不能绕过权限或审批。
10. DAG 工作流由 Rust 调度器负责，Python deepagents 只执行单个节点 run。

## 4. 推荐总体架构

```text
Electron + React Desktop
  - 当前发起端 SSE
  - 其他端 WebSocket subscribe conversation_id
  - local executor device bridge
        |
        v
Rust Backend / Control Plane
  - API Gateway
  - AuthN/session/device
  - FerrisKey OIDC/RBAC adapter
  - Resource Authz PEP/PDP adapter
  - Agent/Tool/Skill/MCP management
  - Run Gateway
  - Event Ingest + Outbox
  - SSE/WS stream gateway
  - Approval service
  - Workflow DAG scheduler
  - File service
  - Memory service
  - Audit service
        |
        +--> FerrisKey OIDC/RBAC source
        +--> Postgres business/events/audit/checkpoint metadata
        +--> Redis Stream projection
        +--> RustFS object/file storage
        +--> Vault/KMS secrets
        |
        v
Python Agent Runtime
  - FastAPI internal API
  - Celery worker
  - deepagents create_deep_agent
  - durable checkpointer
  - platform backend adapter
  - tool wrapper / MCP wrapper
  - event projection to Rust
```

### 4.1 控制面

Rust 后端负责：

- 用户认证、设备会话、多端登录、令牌刷新和撤销。
- 所有外部 API 的租户隔离、FerrisKey token 校验和 Rust 资源级授权。
- 管理 agent、agent version、tool、skill、MCP server、workflow、project、memory policy。
- 创建 conversation、run、workflow run、approval、interrupt。
- 接收 Python 运行事件并分配全局递增 `seq`。
- 将事件写入 Postgres，事务提交后通过 outbox 投递 Redis。
- 暴露 SSE 和 WebSocket 订阅，不让客户端直连 Python 或 Redis。
- 作为 local executor 的唯一调度方。

### 4.2 运行面

Python agent service 负责：

- 接收 Rust internal API 创建的 run。
- 从 Rust 获取运行配置快照，包括 agent version、model、tools、skills、MCP、file backend、memory context、policy version。
- 通过 Celery 执行长任务。
- 创建 deepagents agent，绑定 durable checkpointer。
- 使用 DeepAgents event streaming/stream API 消费 messages、tool calls、subagents、tasks、values。
- 将原始运行事件归一化后回传 Rust `/internal/run-events`。
- 在工具执行前调用 Rust `/internal/tool-calls:authorize` 或使用 Rust 下发的短期 tool execution token。

Python service 不得：

- 直接面对桌面端或 Web 端。
- 直接访问 FerrisKey 作为资源级最终授权入口。
- 直接写企业审计事实源。
- 直接访问 local executor。
- 持久化 RBAC/ReBAC 事实。

### 4.3 事件面

事件写入必须采用事务 outbox：

```text
Python event -> Rust event ingest
  -> BEGIN
  -> insert run_events(conversation_id, run_id, seq, event_id, type, payload)
  -> insert event_outbox(event_id, target = redis/ws)
  -> COMMIT
  -> outbox publisher XADD Redis
  -> SSE/WS gateway fanout
```

这样可以避免“Postgres 已写入但 Redis 丢失”或“Redis 已投递但审计缺失”的不一致。

Redis key 建议：

```text
stream:tenant:{tenant_id}:conversation:{conversation_id}
stream:tenant:{tenant_id}:run:{run_id}
```

Redis event 保留策略：

- 保留最近 N 天或 N 条，满足短期续传。
- 长期回放从 Postgres `run_events` 读取。
- Redis 中的事件可被重建，不作为审计事实源。

### 4.4 文件面

统一文件接口由 Rust File Service 暴露：

```http
GET  /internal/files/read
POST /internal/files/write
POST /internal/files/edit
GET  /internal/files/list
GET  /internal/files/glob
POST /internal/files/search
```

远程文件：

- RustFS 存储对象内容和版本。
- Postgres 保存 project、mount、file metadata、revision、etag、hash、last_writer。
- 写入必须带 `expected_revision`，冲突返回 `409 conflict`。
- 大文件、二进制文件、敏感文件必须支持大小限制、内容扫描和脱敏策略。

本地文件：

- Electron main process 的 local executor 只接收 Rust 后端下发的短期操作令牌。
- 本地路径不直接暴露给 Python。Python 只看到虚拟路径，例如 `/workspace/src/main.rs`。
- Rust 按 project mount 把虚拟路径映射到远程 RustFS 或设备本地路径。
- 本地读写前仍调用 Rust 资源级授权：`file:{project_id}:{path_hash}#read/write`，授权输入包含 FerrisKey token roles。

DeepAgents 对接方式：

- 实现 `PlatformFileBackend`，把 `ls/read_file/write_file/edit_file/glob/grep` 转成 Rust internal file API。
- 对 `/workspace/` 路径走项目文件。
- 对 `/scratch/` 使用 DeepAgents `StateBackend` 或 run-scoped backend。
- 对 `/memories/` 使用只读 memory backend，不允许 agent 直接写长期记忆。
- DeepAgents `FilesystemPermission` 作为路径级防线，默认 deny `/**`，按 run 快照下发 allow 规则。

## 5. FerrisKey 授权设计

### 5.1 当前 FerrisKey 定位

当前本地 FerrisKey 已创建 `bibi-work` realm，并可确认提供：

- OIDC discovery、authorization、token、refresh、revoke、userinfo、JWKS 等认证能力。
- realm、client、user、role、client scope、protocol mapper 等管理能力。
- token 中的 roles/scope/audience 声明能力。

当前未确认 FerrisKey 本地 API 直接提供项目代码中配置的资源级 PDP 端点：

```yaml
ferriskey:
  check_path: /api/v1/authz/check
  batch_check_path: /api/v1/authz/batch-check
```

因此 v1 不应把 `ferriskey.base_url` 直接指向 FerrisKey API 后期待上述端点可用。当前阶段采用分层授权：

```text
FerrisKey:
  AuthN + realm/client/user/role/scope + token roles

Rust Backend:
  JWT 校验 + tenant/session/device 校验
  roles claim 解析
  Postgres 资源关系查询
  resource authz check/batch-check
  audit/tool_calls 记录
```

如果未来 FerrisKey 增加或接入资源级 PDP，可以把 Rust 的 resource authz adapter 后端从本地策略引擎切换为 FerrisKey PDP，但外部 API 合约不应改变。

### 5.2 Client 与 Token 配置

建议在 `bibi-work` realm 下创建：

```text
bibi-work-desktop
  - public client
  - Authorization Code + PKCE
  - Electron desktop 登录

bibi-work-web
  - public client
  - Authorization Code + PKCE
  - Web 登录

bibi-work-backend
  - confidential client
  - Rust 后端服务端回调、introspection 或 service account

bibi-work-runtime
  - confidential/internal client
  - Python agent runtime -> Rust backend 服务间认证
```

Token 建议：

```text
scope: openid profile email roles
roles claim: realm_access.roles 或独立 roles claim
audience: bibi-work-backend
issuer: https://id.example.com/realms/<realm>
```

Rust 后端必须校验：

- `iss` 等于 `bibi-work` realm issuer。
- `aud` 包含 `bibi-work-backend` 或当前 API 允许的 audience。
- `exp/nbf/iat/jti` 有效。
- roles claim 来源可信。
- session/device 未被撤销。

### 5.3 当前阶段角色层级

企业级智能体平台需要支持一个唯一用户账号同时拥有多个部门身份。例如高校场景中，Alice 可以同时是教务处人员、图书馆人员和班主任；她不应有三个账号，而应有一个账号和多个角色/组织关系。

当前建议使用 5 层权限模型：

```text
1. 平台层
   platform_owner
   security_admin
   audit_admin

2. 租户层
   tenant_admin
   tenant_member
   tenant_auditor

3. 组织/部门层
   dept_academic_affairs_member
   dept_academic_affairs_admin
   dept_academic_affairs_approver
   dept_library_member
   dept_library_admin
   dept_library_approver
   class_advisor
   class_advisor_approver

4. 能力层
   agent_admin
   skill_admin
   mcp_admin
   tool_user
   workflow_operator
   memory_manager

5. 资源/执行层
   project_owner/member/viewer
   file_read/file_write
   conversation_participant
   approval_approver
   local_exec_user
```

前 4 层适合进入 FerrisKey role/token，作为列表可见性和粗粒度入口权限。第 5 层是资源实例权限，不应全部塞进 token，应由 Rust 后端根据资源关系和上下文实时判定。

Alice 示例：

```text
user:alice
  roles:
    tenant_member
    dept_academic_affairs_member
    dept_library_member
    class_advisor
    dept_academic_affairs_approver
    tool_user
    workflow_operator

resource relations:
    project:{academic_project_id}#member
    project:{library_project_id}#member
    project:{class_project_id}#owner
    approval:{academic_approval_id}#approver
```

### 5.4 资源命名

以下资源命名用于 Rust resource authz adapter、审计日志和业务资源关系，不要求 FerrisKey 原生支持这些资源类型。

```text
tenant:{tenant_id}
user:{user_id}
org:{org_id}
department:{department_id}
device:{device_id}
session:{session_id}
agent:{agent_id}
agent_version:{agent_version_id}
tool:{tool_id}
skill:{skill_id}
mcp_server:{mcp_server_id}
mcp_tool:{mcp_server_id}:{tool_name}
project:{project_id}
file:{project_id}:{path_hash}
conversation:{conversation_id}
run:{run_id}
workflow:{workflow_id}
workflow_run:{workflow_run_id}
memory:{memory_id}
approval:{approval_id}
audit:{audit_id}
```

### 5.5 动作集合

```text
visible
read
create
update
delete
manage
run
use
execute
subscribe
approve
cancel
resume
write
export
impersonate
```

### 5.6 推荐关系

以下关系属于资源级授权模型，建议保存在 Postgres 业务关系表或专门的 resource authz projection 中。`role:{id}` 引用 FerrisKey 中的稳定 role id 或 role name。

```text
tenant:{id}#admin@user:{id}
tenant:{id}#member@user:{id}
department:{id}#admin@user:{id}
department:{id}#member@user:{id}
department:{id}#approver@user:{id}
project:{id}#owner@user:{id}
project:{id}#member@user:{id}
project:{id}#viewer@user:{id}
agent:{id}#owner@user:{id}
agent:{id}#runner@role:{id}
tool:{id}#user@role:{id}
skill:{id}#user@role:{id}
mcp_tool:{server}:{name}#user@role:{id}
device:{id}#owner@user:{id}
conversation:{id}#participant@user:{id}
workflow:{id}#operator@role:{id}
approval:{id}#approver@user:{id}
```

### 5.7 Rust 资源级授权接口

```http
POST /api/v1/authz/check
POST /api/v1/authz/batch-check
POST /internal/authz/check
POST /internal/authz/batch-check
```

上述接口应由 Rust 后端提供。它可以在内部使用 FerrisKey token roles、Postgres 资源关系和本地策略引擎，也可以在未来代理到 FerrisKey 资源级 PDP。客户端、Python runtime、local executor 不应直接调用 FerrisKey 做资源级授权。

请求模型：

```ts
type AuthzCheckRequest = {
  tenant_id: string;
  actor: {
    user_id: string;
    device_id?: string;
    session_id?: string;
  };
  action: string;
  resource: {
    type: string;
    id: string;
    path?: string;
  };
  context?: {
    project_id?: string;
    conversation_id?: string;
    run_id?: string;
    workflow_run_id?: string;
    agent_id?: string;
    tool_id?: string;
    mcp_server_id?: string;
    args_hash?: string;
    risk_level?: "low" | "medium" | "high" | "critical";
    source_ip?: string;
    user_agent?: string;
  };
};
```

响应模型：

```ts
type AuthzDecision = {
  decision: "allow" | "deny" | "review";
  policy_version: string;
  reason_code?: string;
  obligations?: {
    approval_policy_id?: string;
    approval_timeout_sec?: number;
    audit_level?: "normal" | "high" | "critical";
    redact_fields?: string[];
    max_output_bytes?: number;
    require_mfa?: boolean;
  };
};
```

### 5.8 FerrisKey 配置流程

当前创建 Alice 或其他用户时，建议按以下顺序操作：

1. 创建 `bibi-work-desktop`、`bibi-work-web`、`bibi-work-backend`、`bibi-work-runtime` client。
2. 创建用户，例如 `alice`。
3. 设置用户初始密码，并按需要强制首次修改。
4. 创建 realm roles 和必要 client roles。
5. 给用户分配多个角色，而不是创建多个账号。
6. 配置 client scope/protocol mapper，让角色出现在 access token。
7. Rust 后端读取 token 中的 roles，并对 agent/tool/skill/MCP/file/local-exec 做资源级二次鉴权。

### 5.9 授权缓存规则

- 列表页可使用短期 projection cache 做预过滤。
- 执行型动作必须实时调用 Rust 资源级授权接口，或使用由 Rust 签发且绑定 `actor/resource/action/policy_version/expires_at` 的短期授权令牌。
- FerrisKey 不可用、token 无法校验或资源级授权接口不可用时默认 fail closed。只有只读低风险列表可以在明确配置下使用过期缓存降级。
- 所有授权结果必须记录 `policy_version`，写入 audit 和 tool_calls。

## 6. 运行、审批与恢复

### 6.1 Run 状态机

```text
queued
running
waiting_approval
waiting_user_input
interrupted
cancelling
cancelled
completed
failed
expired
```

状态转换只能由 Rust 后端完成。Celery task 状态只是 worker 视角，不是平台事实源。

### 6.2 创建 run

```text
POST /api/v1/conversations/{conversation_id}/runs:stream
  -> 验证 session/device/tenant
  -> Rust authz: agent:{id}#run
  -> Rust authz: project:{id}#read/use
  -> 保存 run_config_snapshot
  -> insert runs(status=queued)
  -> insert run_events(run.started or run.queued)
  -> enqueue Celery with idempotency_key
  -> 建立 SSE，从 after_seq=0 开始推送
```

`run_config_snapshot` 必须包含：

- agent version。
- model/provider 配置引用。
- skill/tool/MCP 可用列表和 schema hash。
- file backend mount snapshot。
- memory retrieval snapshot。
- FerrisKey issuer/client/scope/roles snapshot。
- Rust resource authz policy version。
- risk policy version。

### 6.3 工具执行

所有工具统一走 wrapper：

```text
before_tool_call
  -> normalize tool name/resource id
  -> calculate args_hash
  -> classify risk_level
  -> Rust authorize_tool_call
  -> allow: execute
  -> review: create approval + interrupt + pause run
  -> deny: return denied ToolMessage

after_tool_call
  -> summarize input/output
  -> redact sensitive fields
  -> build ToolResultView descriptors
  -> persist large result data as artifact/object reference
  -> emit tool.call.completed/failed
  -> write audit
```

工具分类：

- Built-in file tools：通过 `PlatformFileBackend` 调 Rust File Service。
- MCP tools：通过平台 MCP wrapper 调用，禁止 agent 直接持有 MCP client secret。
- Local exec tools：Python 调 Rust Local Execution API，Rust 再路由到设备。
- Memory tools：只允许查询和提交候选记忆，不允许直接写 core/procedural 长期记忆。
- SQL tools：逐库、逐 schema、逐表、逐 query hash 授权。

#### 6.3.1 工具结果 UI 协议

平台不应让工具或 LLM 直接生成任意 React/HTML/JS 作为运行时 UI。工具执行链已经由 DeepAgents -> Python wrapper -> Rust 授权/审计 -> SSE/WS 固化，最简单且正确的增强方式是增加平台自有的工具结果展示协议：

```ts
type ToolResultView =
  | {
      kind: "table";
      title?: string;
      columns: Array<{
        key: string;
        label?: string;
        type?: "string" | "number" | "boolean" | "datetime" | "currency";
      }>;
      rows_preview: Array<Record<string, unknown>>;
      data_ref?: ArtifactRef;
    }
  | {
      kind: "chart";
      title?: string;
      spec_kind: "vega_lite";
      spec: Record<string, unknown>;
      data_ref?: ArtifactRef;
    }
  | {
      kind: "map";
      title?: string;
      format: "geojson";
      data_ref: ArtifactRef;
      data_preview?: Record<string, unknown>;
      style_ref?: string;
    }
  | { kind: "json"; title?: string; value_preview: unknown; data_ref?: ArtifactRef }
  | { kind: "file_diff"; title?: string; files: FileDiffRef[] }
  | { kind: "markdown"; title?: string; text: string }
  | { kind: "artifact"; title?: string; artifact_ref: ArtifactRef };

type ArtifactRef = {
  artifact_id: string;
  object_reference_id?: string;
  content_type: string;
  content_hash: string;
  size_bytes: number;
};
```

协议规则：

- `output_summary` 继续作为审计、列表和折叠卡片摘要，不承载富渲染数据。
- `tool.call.completed` payload 可以携带 `views: ToolResultView[]`；小结果只放 preview，大结果必须写入 `bibi-work-runs` 或 project artifact，并只在事件中放 `ArtifactRef`。
- Rust `/internal/run-events` 必须校验 `views` schema、大小、content type、hash、artifact ownership 和租户/run 归属，校验失败时保留摘要并丢弃富视图；校验通过的 artifact 绑定写入 `tool_result_artifacts`。
- Python `PlatformToolWrapper` 在授权 obligations 脱敏、截断之后再生成 `ToolResultView`，不能把原始 secret、完整敏感参数或未脱敏输出交给前端；工具 spec、`output_schema` 和 vendor 扩展中的 `ui_hints` 会被规范化为 view kind 选择提示。
- 前端使用 renderer registry 按 `kind` 白名单渲染：`table/chart/map/json/file_diff/markdown/artifact`。未知 `kind` fallback 到摘要或 JSON 预览；大对象通过 `/api/v1/tool-result-artifacts/read` 按需读取，表格 artifact 推荐 JSONL。
- 图表只允许声明式 spec，v1 推荐 `vega_lite`；地图只允许 GeoJSON artifact，并可携带小型 `data_preview` 供前端 MapLibre 直接预览；禁止可执行脚本、任意 iframe 和运行时动态组件。
- 每个工具版本可以在 `tool_version.snapshot.output_schema` 与 `ui_hints` 中声明期望输出和默认渲染方式；运行时仍以 Rust 校验后的 `views` 和 `tool_result_artifacts` 绑定为准。

展示层必须与执行事实和模型上下文分离：Rust/Python 保留稳定、可审计的原始结构，LLM 获取保留真实语义的结果，Renderer 只消费脱敏后的展示投影。工具卡优先展示受控 `ToolResultView`，其次展示人类可读的请求字段和确定性结果摘要，原始 JSON 仅放在用户主动展开的技术详情中。`summary` 是审计与折叠体验字段，不是工具执行或模型推理的必填字段，也不应为了生成摘要额外调用一次 LLM。`api_key`、`authorization`、`password`、`secret`、`token`、`credential`、`cookie` 等敏感字段不得进入友好展示。

### 6.4 审批恢复

DeepAgents HITL 需要 durable checkpointer。平台应使用 Postgres/Redis-backed checkpointer 或可恢复的 LangGraph checkpointer，不应在生产使用内存 checkpointer。

审批流程：

```text
tool wrapper receives review
  -> Rust creates approval + interrupt
  -> Python raises/returns HITL interrupt
  -> Celery task persists checkpoint and exits/parks
  -> run status = waiting_approval
  -> approver decision via Rust API
  -> Rust authz checks approval:{id}#approve
  -> Rust calls Python /internal/agent-runs/{run_id}/resume
  -> Python resumes with same thread_id/checkpoint_id
  -> run status = running
```

恢复必须使用同一个 `tenant_id/conversation_id/run_id/thread_id`，否则会产生重复工具调用或丢失上下文。

## 7. DeepAgents 集成方案

### 7.1 Agent 构建

Python service 根据 Rust 下发的配置构建：

```python
agent = create_deep_agent(
    model=model,
    system_prompt=agent_prompt,
    tools=platform_wrapped_tools,
    subagents=compiled_subagents,
    backend=platform_composite_backend,
    permissions=filesystem_permissions,
    interrupt_on=interrupt_config,
    checkpointer=durable_checkpointer,
)
```

### 7.2 Backend 迁移

官方 DeepAgents backend 支持 State、Filesystem、Store、Composite、Sandbox、LocalShell 和自定义 backend。平台应采用自定义 backend：

```text
CompositeBackend
  /workspace/ -> PlatformProjectBackend(Rust File Service)
  /scratch/   -> StateBackend(run scoped)
  /memories/  -> PlatformMemoryReadOnlyBackend
  /policies/  -> PlatformPolicyReadOnlyBackend
```

不建议在企业生产环境中把 `FilesystemBackend(root_dir=...)` 或 `LocalShellBackend` 暴露给 HTTP API。LocalShell 无隔离，只适合受控开发环境。

### 7.3 Permissions

DeepAgents permissions 只覆盖内置文件工具：

- `ls`
- `read_file`
- `glob`
- `grep`
- `write_file`
- `edit_file`

它不覆盖自定义工具、MCP 工具，也不覆盖可执行 shell 的 sandbox backend。因此平台策略是：

1. DeepAgents permissions 默认 deny `/**`。
2. 只按项目 mount 下发允许访问的虚拟路径。
3. 自定义工具、MCP、本地执行、SQL 仍必须通过 Rust 二次校验。
4. permissions 拒绝事件也回传 Rust 写审计。

### 7.4 Subagents

DeepAgents subagents 适合解决上下文膨胀和专家分工。平台的 Agent 管理应把“专家智能体”编译为 subagent spec：

```ts
type PlatformSubagentSpec = {
  name: string;
  description: string;
  system_prompt: string;
  model_ref?: string;
  tool_ids: string[];
  skill_ids: string[];
  permissions_profile_id?: string;
  response_schema?: Record<string, unknown>;
};
```

关键规则：

- subagent 的 tools/skills 必须由 Rust 资源级授权对当前 actor、FerrisKey roles 和 agent version 判定。
- subagent 可以继承主 agent 权限，但企业平台建议显式生成子权限快照。
- subagent 事件必须映射为 `subagent.started/subagent.message.delta/subagent.tool.call/subagent.completed`。
- 长任务、多节点并发不要只依赖 synchronous subagent；DAG 工作流由 Rust 调度器编排。

### 7.5 Streaming

DeepAgents 支持从主 agent 和 subagent 流式输出 messages、tool calls、updates 等事件。平台应在 Python 内做一次归一化：

```text
DeepAgents raw stream
  -> AgentEventNormalizer
  -> PlatformStreamEvent
  -> Rust /internal/run-events
  -> Postgres + Redis
  -> SSE/WS
```

标准事件类型：

```ts
type StreamEventType =
  | "run.queued"
  | "run.started"
  | "message.delta"
  | "message.completed"
  | "tool.call.started"
  | "tool.call.delta"
  | "tool.call.completed"
  | "tool.call.failed"
  | "interrupt.requested"
  | "approval.requested"
  | "approval.completed"
  | "task.created"
  | "task.updated"
  | "task.completed"
  | "subagent.started"
  | "subagent.message.delta"
  | "subagent.tool.call.started"
  | "subagent.completed"
  | "file.changed"
  | "workflow.node.queued"
  | "workflow.node.running"
  | "workflow.node.completed"
  | "workflow.node.failed"
  | "run.completed"
  | "run.failed"
  | "run.cancelled";
```

`tool.call.completed` 的 payload 应扩展为“摘要 + 可选结构化视图”：

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
      "columns": [{ "key": "region", "label": "区域", "type": "string" }],
      "rows_preview": [{ "region": "华东" }],
      "data_ref": {
        "artifact_id": "uuid",
        "content_type": "application/json",
        "content_hash": "sha256:...",
        "size_bytes": 128000
      }
    }
  ]
}
```

该扩展不改变事件事实源：Postgres `run_events` 仍是回放依据，Redis/SSE/WS 仍只是实时投递；富结果对象由 RustFS/object reference 承载。

## 8. 多端登录与流同步

当前 `user_token` 使用 `(user_id, platform)` 唯一，不支持同平台多设备并发。建议调整：

```text
devices
- id
- tenant_id
- user_id
- device_name
- platform
- public_key
- trust_level
- last_seen_at
- revoked_at

sessions
- id
- tenant_id
- user_id
- device_id
- session_jti
- refresh_token_hash
- expires_at
- revoked_at
- created_at

user_token
- 保留兼容，或迁移为 session token projection
```

SSE：

- 当前发起端使用。
- 支持 `Last-Event-ID`。
- 断线后从 Postgres 回放缺失事件，再接 Redis 实时流。

WebSocket：

- 其他端通过 `conversation_id` 订阅。
- 订阅前检查 `conversation:{id}#subscribe`。
- 支持 `after_seq`。
- 支持 session revoke 后强制断开。
- 支持心跳和 backpressure。

订阅消息：

```json
{ "op": "subscribe", "conversation_id": "conv_1", "after_seq": 120 }
```

## 9. RustFS 与本地执行

### 9.1 RustFS 配置与职责

RustFS endpoint、凭据和本机初始化状态属于部署配置，不写入架构文档。平台要求相关业务 bucket 启用 versioning，并通过 Rust File/Object Service 统一访问。逻辑 bucket 规划如下：

| bucket | 职责 |
| --- | --- |
| `bibi-work-marketplace` | MCP 工具市场、Skill 市场、agent templates、策略包 |
| `bibi-work-files` | 部门共享文件、项目文件、重要资源、用户远程目录空间 |
| `bibi-work-runs` | conversation/run/workflow 运行产物、scratch、上传和导出 |
| `bibi-work-memory` | 四层记忆相关文件、导出、ingestion 中间产物 |
| `bibi-work-audit` | 审计事件、授权决策、工具调用、审批、hash chain、合规导出 |

RustFS 只负责对象存储、版本化对象和持久文件内容，不是权限边界。所有客户端、Python runtime 和 local executor 都不得直接拿 RustFS 凭据访问对象。平台必须通过 Rust File Service 访问 RustFS，并在 Rust 层完成：

- tenant/project/user/department 路径映射。
- FerrisKey token roles 校验。
- 资源级 `file/project/memory/audit` 授权。
- revision/etag 冲突控制。
- 内容大小、敏感信息和路径逃逸检查。
- file revision、run event 和 audit 记录。

### 9.2 Bucket 与目录层级

当前目录规划：

```text
bibi-work-marketplace/
  global/mcp-marketplace/
  global/skill-marketplace/
  tenants/bibi-work/mcp-marketplace/
  tenants/bibi-work/skill-marketplace/
  tenants/bibi-work/agent-templates/
  tenants/bibi-work/policies/

bibi-work-files/
  tenants/bibi-work/departments/academic-affairs/
  tenants/bibi-work/departments/library/
  tenants/bibi-work/departments/student-affairs/class-advisors/
  tenants/bibi-work/projects/
  tenants/bibi-work/resources/important-files/
  tenants/bibi-work/users/_template/remote-space/

bibi-work-runs/
  tenants/bibi-work/conversations/
  tenants/bibi-work/runs/
  tenants/bibi-work/workflow-runs/
  tenants/bibi-work/artifacts/
  tenants/bibi-work/scratch/

bibi-work-memory/
  tenants/bibi-work/users/_template/core-profile/
  tenants/bibi-work/users/_template/episodic/
  tenants/bibi-work/users/_template/semantic/
  tenants/bibi-work/users/_template/procedural/

bibi-work-audit/
  tenants/bibi-work/audit/events/
  tenants/bibi-work/audit/authz-decisions/
  tenants/bibi-work/audit/tool-calls/
  tenants/bibi-work/audit/approvals/
  tenants/bibi-work/audit/hash-chain/
```

命名规则：

- tenant 使用稳定 slug，例如 `bibi-work`。
- 部门使用稳定英文 slug，中文显示名放在 Postgres metadata。
- 项目目录下一层使用不可变 `project_id`，项目名称放在 metadata。
- 用户目录必须使用不可变 `user_id`，不要使用 username、邮箱或姓名。
- `_template` 仅用于初始化骨架，不代表真实用户目录。

真实用户目录示例：

```text
bibi-work-files/tenants/bibi-work/users/{user_id}/remote-space/
bibi-work-memory/tenants/bibi-work/users/{user_id}/core-profile/
bibi-work-memory/tenants/bibi-work/users/{user_id}/episodic/
bibi-work-memory/tenants/bibi-work/users/{user_id}/semantic/
bibi-work-memory/tenants/bibi-work/users/{user_id}/procedural/
```

### 9.3 RustFS 远程文件

文件写入模型：

```ts
type FileWriteRequest = {
  project_id: string;
  path: string;
  content_ref?: string;
  inline_content?: string;
  expected_revision: number;
  reason: "agent_edit" | "user_edit" | "workflow_output";
  run_id?: string;
};
```

每次写入：

- Rust 资源级授权检查 `file:{project_id}:{path_hash}#write`，授权输入包括 FerrisKey token roles。
- 校验路径不能逃逸 project root。
- 校验 `expected_revision`。
- 写 RustFS 新对象。
- 写 `file_revisions`。
- 写 `file.changed` 事件。
- 写 audit。

对象 key 建议：

```text
bibi-work-files/tenants/{tenant_slug}/projects/{project_id}/workspace/{normalized_path}
bibi-work-runs/tenants/{tenant_slug}/runs/{run_id}/artifacts/{artifact_id}
bibi-work-audit/tenants/{tenant_slug}/audit/file-revisions/{yyyy}/{mm}/{dd}/{event_id}.json
```

Postgres 保存业务索引和一致性元数据：

```text
project_mounts
file_revisions
object_key
bucket
etag
version_id
content_hash
expected_revision
last_writer_user_id
source_run_id
```

文本搜索不把任意大对象直接写入单个 `tsvector`。Rust File Service 在写 revision 时生成 `file_search_chunks`：

- 不超过 1 MiB 的文本按 64 KiB UTF-8 安全边界全量分块。
- 超过 1 MiB 的文本在文件头、中心、尾部及其间位置均匀采样最多 1 MiB，避免只索引前缀。
- 每个 chunk 记录 byte range、源大小、实际索引字节、截断状态和 `full_chunks | uniform_sample` 策略。
- `file_revisions.metadata.search_index` 保存相同抽取摘要，便于审计和前端解释搜索结果。
- 大文本搜索只返回最佳命中 chunk 作为截断 preview；完整内容继续通过 File Service byte range 或 raw stream 读取。
- 二进制正文不进入全文索引，但仍可按虚拟路径检索，不会因此读取或返回二进制内容。

### 9.4 本地执行协议

Python 不直接执行本地命令。流程：

```text
Python tool local_execute
  -> Rust /internal/local-exec/requests
  -> Rust authz: device:{id}#use + project:{id}#run_local
  -> risk classifier
  -> approval if needed
  -> Rust sends command to local executor over device websocket
  -> local executor validates token/path/cwd
  -> execute with timeout/output cap
  -> stream stdout/stderr chunks to Rust
  -> Rust persists events/audit
  -> result returned to Python tool
```

local executor 必须具备：

- 设备绑定和公钥认证。
- 每次操作短期令牌。
- cwd/workspace 白名单。
- 路径 realpath 防逃逸。
- 禁止危险命令或强制审批。
- 进程取消。
- 超时和输出限流。
- stdout/stderr 脱敏摘要。
- 本地文件变更上报。

可借鉴 Open Cowork 的路径守卫、WSL/Lima 沙盒、权限弹窗、MCP 管理和 Skill 管理，但企业版必须把审批和授权上移到 Rust + FerrisKey，不能只依赖桌面端本地状态。

## 10. Agent、Skill、MCP 与工具管理

### 10.1 版本化模型

```text
agents
agent_versions
skills
skill_versions
tools
tool_versions
mcp_servers
mcp_tools
agent_version_skill_bindings
agent_version_tool_bindings
agent_version_mcp_bindings
```

运行时只使用 version 快照，不使用 mutable draft。

### 10.2 管理流程

```text
Admin creates/updates agent draft
  -> Rust validates schema
  -> Rust writes business metadata
  -> Rust writes resource visibility/use policy bindings
  -> optional: Rust maps coarse-grained availability to FerrisKey roles/scopes
  -> publish version
  -> future runs bind agent_version_id
```

Skill：

- 支持内置 skill、租户级 skill、项目级 skill。
- Skill 内容必须有 manifest、版本、hash、签名或来源记录。
- Skill 只能作为 prompt/tool 配置输入，不允许绕过工具授权。
- Skill package 和 manifest 建议存储在 `bibi-work-marketplace/tenants/bibi-work/skill-marketplace/` 下，Postgres 保存可检索元数据、版本、hash 和发布状态。

MCP：

- MCP server 配置密钥进入 Vault/KMS，不进普通 Postgres 明文。
- discover 结果写 `mcp_tools` 和 schema hash。
- MCP `tools/list` 是工具目录的权威快照：每次成功发现先停用旧工具，再按 `(mcp_server_id, name)` 恢复本次返回项，服务端已删除的工具不会继续参与 capability snapshot。`mcp_servers` 使用结构化 health 列记录 healthy/unhealthy/unsupported、检查时间、最近成功发现、连续失败次数和有界错误；BiWork 的连接检查与 public catalog discover 共用这一生命周期事实。
- 后台 MCP health worker 只探测 active 的 `http/json-rpc/streamable-http` 服务；按最久未检查优先取有界批次，最多 8 路并发，missed tick 直接跳过，不让慢服务形成无限积压。stdio 仍由 actor device 的 Electron runtime 负责，服务端不伪造本地进程可达性。
- stdio MCP 使用 BiWork desktop gateway 与 Rust catalog 的 `FACADE` 边界：Electron 主进程通过官方 MCP SDK、无 shell `spawn` 完成 initialize/tools-list，只把规范化工具观测结果回写 Rust；Rust 再做 tenant/authz、transport 校验、schema hash、权威目录同步和 health 持久化。工具执行时 Rust 从 `mcp_tools` 事实加载真实 server/tool/schema，按 `readOnlyHint/destructiveHint` 判定风险并完成资源授权，然后把不含明文 secret 的 `mcp_stdio` work item 定向到 actor device；Electron main 只领取同设备同 kind 请求，用官方 SDK 执行并回报。stdio `env` 只允许 `env://NAME`，由 Electron 进程环境解析，明文值不能进入 Rust `config`、renderer、queue 或 discovery report。
- streamable HTTP MCP 由 Rust transport 统一完成 initialize、`notifications/initialized`、`Mcp-Session-Id`/protocol header、JSON/SSE response 解码、15 分钟 session 复用、失效重建和一次重试；session slot 与响应大小有硬上限。legacy SSE transport 在完成 GET message-endpoint negotiation 前明确 fail closed，不与 streamable HTTP 混用。
- 每次调用前由 Rust 资源级授权检查 `mcp_tool:{server_id}:{tool_name}#use`。
- stdio MCP 只能运行在受控 runtime 或本地 executor，不允许任意用户配置任意命令后由服务器直接执行。
- OAuth token 需要 scope、过期、撤销和审计。
- MCP 市场 catalog、package 和 schema 建议存储在 `bibi-work-marketplace/tenants/bibi-work/mcp-marketplace/` 下；全局默认市场资源存储在 `bibi-work-marketplace/global/` 下。

Agent template 和策略包：

- Agent template 存储在 `bibi-work-marketplace/tenants/bibi-work/agent-templates/`。
- 策略包、risk policy、resource authz policy snapshot 存储在 `bibi-work-marketplace/tenants/bibi-work/policies/`。
- 运行时只使用已发布版本和内容 hash，不直接读取 mutable draft。

## 11. 四层记忆系统

参考 Hermes Agent 的思想，平台采用用户级四层记忆：

1. `core_profile`：身份、长期偏好、表达风格、固定约束。
2. `episodic`：会话摘要、任务经历、关键决策、审批偏好。
3. `semantic`：跨项目知识、文档摘要、领域概念、经验总结。
4. `procedural`：工具选择偏好、工作流习惯、常用 skill、执行策略。

数据模型：

```text
memory_items
- id
- tenant_id
- user_id
- agent_id nullable
- project_id nullable
- layer
- content
- content_hash
- source_run_id
- source_event_id
- confidence
- status: candidate|active|rejected|archived|deleted
- visibility: private|agent|project|tenant
- retention_policy
- sensitivity
- created_at
- updated_at
- deleted_at

memory_embeddings
memory_ingestion_jobs
memory_access_logs
memory_feedback
```

记忆写入流程：

```text
run completed
  -> memory ingestion job
  -> extract candidate memories
  -> deduplicate/confidence score
  -> policy check
  -> auto-activate low-risk procedural/episodic or require user review
  -> write memory_items
  -> embed semantic/episodic
  -> audit memory update
```

记忆注入：

- run 创建前由 Rust memory service 检索。
- Rust 资源级授权检查 `memory:{id}#read`，授权输入包含 FerrisKey token roles。
- 注入到 prompt 的位置必须标记为 untrusted context。
- 对 memory 内容进行 prompt injection 过滤。
- 允许用户查看、编辑、删除、禁用记忆。
- 所有 memory access 写 `memory_access_logs`。

## 12. DAG 工作流

工作流分为设计态、编译态、运行态：

```text
workflow_designs
workflow_nodes
workflow_edges
workflow_versions
workflow_runs
workflow_node_runs
workflow_run_dependencies
workflow_run_events
```

编译规则：

- 保存设计时允许草稿不完整。
- 运行前编译并校验：无环、节点完整、agent 可运行、skill/tool 可用、项目可访问。
- 每个 workflow node 编译为一个独立 `workflow_node_run` 和一个 agent `run`。
- Rust 调度器负责 ready queue、并发、重试、取消、超时和依赖传播。
- Python deepagents 只执行单节点任务。

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

Ordinus 的可视化 DAG、compile-design、workboard、observability 模型值得迁移，但企业版应把本地 Electron IPC 合约改造为 Rust API 合约，把本地 SQLite 状态改为 Postgres 事实源。

## 13. 审计与可观测性

### 13.1 审计日志

```text
audit_logs
- id
- tenant_id
- actor_user_id
- actor_device_id
- session_id
- resource_type
- resource_id
- action
- decision
- policy_version
- reason_code
- run_id
- conversation_id
- workflow_run_id
- tool_call_id
- approval_id
- args_hash
- input_summary
- output_summary
- risk_level
- ip
- user_agent
- trace_id
- prev_hash
- row_hash
- created_at
```

高风险操作必须记录：

- 工具名和资源 ID。
- 参数 hash 和脱敏摘要。
- 输出 hash 和脱敏摘要。
- 审批策略、审批人、审批结果。
- FerrisKey issuer/client/scope/roles snapshot。
- Rust resource authz policy version。
- local executor device id。

审计表采用 UTC 月度 range partition、WORM 归档和链式 hash，防止被无声篡改。`audit_log_identities` 独立保留全局 UUID 身份，使叶子分区可 detach，同时维持 segment 首尾记录的引用完整性。当前 Rust 实现采用 seal/archive 两阶段：seal 固化连续 hash-chain manifest，archive worker 对 RustFS evidence 做 content hash 与 manifest 双重校验、记录归档状态和 retention deadline，并用 `FOR UPDATE SKIP LOCKED` 支持多实例安全 claim；legal hold 支持 tenant、segment、resource scope。到期资格按整个叶子分区 fail closed：任意未验证、未到期、受 hold 保护或未被合格 segment 覆盖的记录都会阻止清理。partition maintenance worker 会预创建当前与未来月份分区，默认分区有冲突数据时拒绝建分区。cleanup API 默认 dry-run、要求 `platform_admin` 且服务端执行开关默认关闭；执行时持有全局 advisory lock 和父表 `ACCESS EXCLUSIVE` lock，二次校验后才 detach/drop。

### 13.2 可观测性

全链路使用 OpenTelemetry：

- `trace_id` 从 Rust API 创建 run 时生成。
- Rust、Python、Celery、Redis publisher、local executor 都传递 `trace_id/run_id/conversation_id`。
- metrics 包括 run latency、tool latency、approval latency、Redis lag、outbox lag、Celery queue depth、worker failure、FerrisKey OIDC/JWKS latency、resource authz latency。
- 日志必须脱敏，不能输出密钥、完整 prompt 中的敏感数据或文件完整内容。

## 14. 数据表建议

第一阶段最小可落地表：

```text
tenants
users
user_tenant_memberships
devices
sessions
ferriskey_role_projection
resource_relations
resource_policy_bindings
authz_decisions
agents
agent_versions
skills
tools
mcp_servers
mcp_tools
projects
project_mounts
conversations
runs
run_events
event_outbox
tool_calls
approvals
interrupts
audit_logs
file_revisions
object_references
```

说明：

- `ferriskey_role_projection` 只缓存 FerrisKey role/client/scope 的只读投影，用于列表预过滤和诊断，不作为最终身份事实源。
- `resource_relations` 保存 project/file/agent/tool/skill/MCP/workflow/approval 等资源实例关系。
- `resource_policy_bindings` 保存角色到资源动作的绑定和策略版本。
- `authz_decisions` 保存 Rust resource authz check/batch-check 的结果、输入摘要、policy version 和 obligations。
- `object_references` 保存 RustFS bucket、object key、etag、version_id、content_hash 与业务资源的映射。

第二阶段：

```text
memory_items
memory_embeddings
memory_ingestion_jobs
memory_access_logs
workflow_designs
workflow_versions
workflow_runs
workflow_node_runs
workflow_run_dependencies
workflow_run_events
local_exec_requests
local_exec_events
```

不要恢复一套与 FerrisKey 竞争的用户/角色事实源。FerrisKey 负责用户、realm roles、client roles、scope 和 token claims；Postgres 负责平台业务资源关系、资源策略绑定和授权审计。资源级权限事实以 Rust resource authz adapter 的策略版本和 Postgres 资源关系为准，粗粒度角色事实以 FerrisKey 为准。

## 15. 分阶段落地路线

### 阶段 0：收敛基础模型

已完成或已确认：

- FerrisKey `bibi-work` realm 已创建。
- RustFS 五个业务 bucket 已创建并启用 versioning。
- 本地服务配置结构见 `docs/local-service-config.md`，真实值只保存在本机忽略文件和 `.env.local`。

下一步：

1. 调整 token/session/device 模型，支持多设备并发和独立撤销。
2. 增加 tenant_id 贯穿所有业务表。
3. 接入 FerrisKey OIDC/JWKS 校验和 roles claim 解析。
4. 实现 Rust resource authz check/batch-check，所有未接入授权的敏感 API fail closed。
5. 增加 audit_logs 和 trace_id。

### 阶段 1：Run Gateway 与事件面

1. 增加 conversations/runs/run_events/event_outbox。
2. 实现 Rust internal event ingest。
3. 实现 outbox publisher 到 Redis Stream。
4. 实现 SSE `Last-Event-ID` 和 WebSocket `after_seq`。
5. Python service 只发事件到 Rust。

### 阶段 2：DeepAgents 运行态

1. Python FastAPI internal API。
2. Celery worker + durable checkpointer。
3. PlatformFileBackend。
4. Tool wrapper + Rust 资源级二次校验。
5. DeepAgents stream normalization。
6. ToolResultView 协议、Python presenter、Rust `views` 校验和 artifact 引用。
7. task/subagent UI 事件映射。

### 阶段 3：审批与高风险工具

1. approvals/interrupts/tool_calls 完整状态机。
2. approval decision resume。
3. 高风险命令、文件写入、MCP、SQL 全部接入 review。
4. 审批超时、撤销、重复提交幂等。

### 阶段 4：文件与本地执行

1. 接入已初始化的 RustFS buckets：marketplace/files/runs/memory/audit。
2. 实现 RustFS remote project mount 和 object_references。
3. 实现 file revision/etag/version_id/conflict。
4. Electron local executor 设备绑定。
5. 本地执行 WS 通道、短期令牌、路径守卫、取消、超时、输出限流。
6. 本地文件变更同步和审计。

### 阶段 5：记忆与工作流

1. 四层记忆存储、检索、注入、候选审核。
2. memory ingestion job。
3. workflow design/version/compile/run。
4. DAG 调度器、节点状态、重试和可视化。
5. workflow/subagent/task 统一事件面。

## 16. 最佳实践检查

当前方案符合的最佳实践：

- 身份与角色事实源收敛到 FerrisKey。
- 资源级授权入口收敛到 Rust 后端。
- Rust 控制面作为 PEP。
- Python agent runtime 内部化。
- Redis 不作为审计事实源。
- 工具调用前二次校验。
- 记忆作为非可信上下文。
- DAG 与 conversation 分离。

v1 必须补齐后才算生产级最佳实践：

- 多租户与设备 session 模型。
- 服务间 mTLS 或签名 JWT。
- Postgres outbox 到 Redis。
- durable checkpointer 和审批恢复。
- Celery 幂等和重复事件去重。
- 工具结果结构化视图协议、对象引用和前端白名单 renderer。
- local executor 零信任协议。
- Vault/KMS 管理 MCP 和模型密钥。
- Rust `SecretResolver` 统一解析 `env://NAME`、`vault://path#field`、`kms://key-id#ciphertext`；Vault/KMS 控制凭证仅来自进程环境，业务 secret 不进入 Postgres 明文、runtime snapshot、BiWork renderer 或日志。KMS 通过受控 decrypt gateway 返回 `plaintext_base64`，便于在云厂商 KMS、HSM 或企业密钥服务前保持稳定内部契约。
- SQL `tls_config_ref` 也走相同 resolver，只接受 require/verify-ca/verify-full 和有界 PEM；LLM runtime credential 仅存 Redis 10 分钟，并在 credential rotate/revoke 时主动删除。
- 自动轮换使用固定 rotation gateway，不接受数据库中的任意 endpoint。Rust worker 通过 `FOR UPDATE SKIP LOCKED` claim 到期 credential，以 attempt UUID 作为 gateway 幂等键；进程在 gateway 成功后、数据库提交前崩溃时，陈旧 claim 会复用原 attempt UUID，避免重复生成外部密钥。轮换期间阻止新 runtime credential 签发并先撤销旧凭证。成功后原子更新 opaque ref 与 hash-chain 审计；失败保留旧 active credential、指数退避，并通过 health/attempt API 暴露告警，不记录原始引用。
- 审计不可篡改和脱敏。
- OpenTelemetry 全链路追踪。
- 容量、灾备、测试和安全威胁建模。

## 17. 相关项目能力迁移建议

### Open Cowork

可迁移：

- 权限弹窗交互。
- 路径守卫和 workspace containment。
- WSL/Lima 沙盒思路。
- MCP server 生命周期和工具发现。
- Skill discovery、manifest、热加载。
- core/experience memory 的提取和检索思路。
- 远程控制和任务调度 UI 思路。

需要改造：

- Electron 本地状态迁移为 Rust API + Postgres。
- 本地审批迁移为 Rust approval service。
- 本地 sandbox 决策迁移为 device-bound local executor。
- MCP/Skill 可见性由 FerrisKey 粗粒度 roles 控制，可用性和执行权由 Rust 资源级授权控制。

### Ordinus

可迁移：

- DAG 可视化模型。
- workflow design 编译为执行计划。
- workboard/task 状态展示。
- runtime boundary 原则。
- observability 事件列表和诊断 UI。

需要改造：

- IPC 合约改为 Rust REST/WebSocket/SSE 合约。
- SQLite 持久化改为 Postgres。
- 本地 provider adapter 改为 Python deepagents runtime。
- 单机 agent room 改为多租户 conversation/run。

### Openwork

可迁移：

- DeepAgents checkpointer/thread 模型。
- tasks/todos/subagents/workspace 文件目录 UI。
- HITL interrupt 恢复 UI。
- DeepAgents backend 和 local sandbox 的组合方式。

需要改造：

- 不能让 deepagents 直接访问真实本地文件。
- 不能把 checkpointer 文件放在单机目录作为企业事实源。
- 不能把 interrupt 只作为前端状态，必须纳入 Rust approval/interrupt 表。

## 18. 官方 DeepAgents 约束

需要在实现中固定的约束：

- Backends 是 DeepAgents 文件工具的抽象层，生产平台应优先使用 custom backend 或 sandbox backend，而不是直接把本机文件系统暴露给 HTTP API。
- FilesystemBackend 直接读写真实文件，有密钥泄露和不可逆写入风险；`virtual_mode=True` 才能提供路径限制。
- LocalShellBackend 没有隔离，只适合受控开发环境。
- Permissions 只覆盖内置文件工具，不覆盖自定义工具、MCP 工具和 shell execute。
- HITL 需要 checkpointer 才能暂停和恢复。
- Subagents 适合上下文隔离和专家分工，但同步 subagent 不等价于企业 DAG 调度器。
- DeepAgents streaming 可以输出主 agent、subagent、LLM token、tool call 和 updates，平台应统一映射为自己的 StreamEvent。

参考：

- https://docs.langchain.com/oss/python/deepagents/backends
- https://docs.langchain.com/oss/python/deepagents/permissions
- https://docs.langchain.com/oss/python/deepagents/human-in-the-loop
- https://docs.langchain.com/oss/python/deepagents/subagents
- https://docs.langchain.com/oss/python/deepagents/streaming

## 19. 最终建议

1. 先实现 `tenant/device/session + FerrisKey OIDC/RBAC + Rust resource authz + audit + run_events + outbox`。
2. 再接 Python deepagents runtime，并强制所有文件和工具调用走 Rust。
3. 再实现审批恢复和本地执行。
4. 最后扩展四层记忆和 DAG 编排。

这样可以尽早把最高风险的权限、审计、事件一致性和本地执行边界固化下来，避免后期在已经接入大量工具和智能体后再补安全模型。
