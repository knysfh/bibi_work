# 企业级智能体运行平台 UI 开发文档

更新日期：2026-06-22

## 1. 文档目标

本文档面向 `Tauri + React + TypeScript` 桌面端 UI 的落地实现，目标是在现有 Rust/Python 后端能力之上，构建一个高内聚、低耦合、可分阶段交付的企业级智能体系统 UI。

当前只实现桌面端。多端能力只在信息架构、状态模型和 API adapter 中保留扩展点，不在本阶段实现移动端或 Web 端。

成功标准：

1. 桌面端只通过 Rust 后端 public API 与受控 Tauri commands 访问平台能力，不直连 Python、Redis、RustFS、MCP server 或本地命令执行器。
2. 前端按 feature-based DDD 拆分，组件不直接 `fetch` 或 `invoke`；所有后端和 Tauri 调用必须经过 adapter layer。
3. 使用 XState 管理运行、审批、登录、工作流编辑/执行、本地执行等流程状态；使用 TanStack Query 管理 server state；使用 Tauri event bus 承接系统级和本地执行事件。
4. UI 能覆盖企业智能体平台核心闭环：OIDC 登录、项目/文件浏览、对话运行、流式事件、task/subagent 展示、审批、agent/skill/tool/MCP/LLM 管理、资源策略、四层记忆治理、工作流 DAG 可视化、审计和设备会话管理。
5. 当前 UI 文案支持中文和英文两种语言；新增页面不得把用户可见文案硬编码在组件内，后续扩展语言只新增 message resource。
6. 每个阶段都有明确验收项，能用单元测试、组件测试、adapter contract 测试和桌面 E2E 验证。

## 2. 输入依据与当前事实

### 2.1 已分析的本仓库资料

- `docs/enterprise-agent-platform-backend-development-plan.md`
- `docs/enterprise-agent-platform-architecture.md`
- `bibi_work_backend/src/features/agent_platform/mod.rs`
- `bibi_work_backend/src/features/agent_platform/models.rs`
- `bibi_work_backend/src/features/agent_platform/handlers.rs`
- `bibi_work_agent` 当前 runtime、tool wrapper、memory、workflow、checkpointer 相关模块

`bibi_work_frontend` 已完成第一轮从零搭建：当前包含 `Tauri 2 + React + TypeScript + Vite` 工程骨架、feature-based DDD 目录、HTTP/Tauri adapter layer、核心后端 DTO contracts、App Shell、工作台/审批/项目文件/设备会话基础 UI，以及 adapter contract 和 run event projection 测试。

当前实现仍是桌面端优先。浏览器 dev 模式只作为 renderer 调试入口；完整桌面安全能力以后续 Tauri runtime、FerrisKey redirect 配置和后端联调为准。

### 2.2 后端可依赖能力摘要

Rust public API 已覆盖：

- OIDC 配置、当前用户、tenant、device、session、logout。
- authz check/batch-check、policy bindings、audit hash-chain verify/seal。
- Agent/Skill/Tool/MCP/LLM catalog 的基础 CRUD、版本、发布、禁用、AgentVersion binding、validate、effective-capabilities。
- Project、project mount、public project file read/list/search/history/artifacts。
- Conversation、run stream、run list/detail/cancel、conversation events、SSE、WebSocket。
- Approval list/decision。
- Memory list/search/upsert/activate/reject/archive/batch-decision。
- Workflow design/version/run/node-runs/cancel/validate。

重要限制：

- Rust 已废弃本地 JWT 登录；桌面端必须实现 FerrisKey OIDC Authorization Code + PKCE。
- public file API 当前只读；直接写文件、编辑、lock 仍是 internal file service 能力。桌面端第一阶段应把文件编辑作为“智能体/本地执行任务产生的变更与预览”，不要伪造浏览器直写能力。
- local exec 当前通过 Rust internal `/internal/local-exec/requests` 建模；桌面端需要后续实现受控本地 executor bridge，不允许 renderer 直接 spawn 命令。
- session/device revoke 后后端仍缺主动关闭已有 SSE/WebSocket 连接；前端必须在事件连接层做 401/403/revoke 后的主动收敛。

### 2.3 参考项目提炼

Open Cowork 可借鉴：

- 三栏工作台、任务历史、权限弹窗、配置面板、MCP 管理、本地 workspace path containment、模型配置体验。
- 必须改造：权限事实源不能在 renderer；审批必须落 Rust `approvals/audit_logs`；MCP secret 不进入本地普通配置；本地命令不能由前端直接执行。

Ordinus 可借鉴：

- DAG canvas、节点字段校验、边作为依赖唯一事实源、编译前 run gating、纯函数 `design -> compiled plan`。
- 必须改造：Electron IPC/SQLite 改为 Tauri adapter + Rust API；节点执行由 Rust workflow scheduler 与 Python runtime 完成。

Openwork 可借鉴：

- 对话中心区 + 右侧 tasks/files/subagents 运行观测面板。
- deepagents events 映射到 todos/tasks、subagents、tool calls、workspace files、interrupt 的 UI 模型。
- 文件 viewer tabs、工具调用折叠卡片、task progress、subagent 状态卡片。
- 必须改造：sql.js checkpoint、本地文件和本地 shell 只能作为参考 UI，不作为企业平台边界。

样式参考 HTML 可借鉴：

- 工作型、低噪声、浅色中性色为主。
- 左侧 260px 左右导航，顶部 58px 工具栏，中间工作区，右侧上下文面板。
- 8px 左右圆角、紧凑行高、清晰边界、可扫描列表，不做营销式 hero。

### 2.4 多角色视角下的核心判断

资深项目架构师视角：

- UI 必须围绕后端已形成的“Rust 控制面 + Python 运行面 + Postgres 事实源 + RustFS 对象存储”设计，而不是把桌面端做成新的事实源。
- 最核心的产品闭环不是 catalog 管理页，而是“创建任务 -> 运行事件 -> 工具/文件/审批 -> 审计/记忆沉淀 -> 下次运行更个性化”。
- 多端架构可以预留，但当前桌面端要先把登录、设备、运行流、审批和本地执行边界做扎实。

Rust 专家视角：

- Tauri Rust side 只承接本机能力和安全存储，不复制后端资源授权逻辑。
- 所有高风险行为都应回到 Rust 后端 public/internal 边界对应的状态机和审计链，而不是在 renderer 或 Tauri command 中自行判定。
- 本地 executor 是高风险扩展面，必须按 device-bound request、短期 token、path containment、输出限流和 fail closed 实现。

Tauri 专家视角：

- 桌面端 OIDC 应使用 PKCE + deep link/loopback callback，token 放 OS secure storage。
- `invoke` 必须有显式能力白名单和 adapter 封装；renderer 不应持有可直接执行命令、读写任意路径的通用 command。
- Tauri event bus 适合承载系统级事件和本地执行器事件，不适合替代后端 SSE/WS 的平台运行事件事实源。

React 设计专家视角：

- XState 管流程状态，TanStack Query 管服务端事实，组件只消费投影后的 view model。
- 工作台采用三栏高密度操作界面，比“主页式入口 + 卡片堆叠”更符合企业智能体平台的重复工作场景。
- 右侧 inspector 是任务、subagent、文件变更、审批和记忆候选的统一观察面，能降低用户在多个页面之间跳转的成本。

## 3. 范围与非目标

### 3.1 当前范围

桌面端必须覆盖：

1. 应用启动、OIDC 登录、token/session/device 投影、退出。
2. 平台主工作台：项目、会话、运行、流式消息、tool call、task、subagent、审批。
3. 项目文件只读浏览、搜索、历史、产物查看。
4. Agent/Skill/Tool/MCP/LLM 管理与版本发布。
5. Policy binding 可视化管理。
6. 四层记忆治理台。
7. Workflow DAG 设计、校验、发布、运行、节点状态可视化。
8. 审批中心、审计查询入口、hash-chain verify/seal。
9. 设备与会话管理。
10. 本地 executor 的桌面 bridge 协议和 UI 状态，但命令执行必须经 Rust 授权与审批。
11. 前端 UI 国际化：当前支持 `zh-CN` 与 `en-US`，语言切换属于本地 UI 偏好，不影响后端权限、审计或业务事实。

### 3.2 非目标

- 不实现移动端 UI。
- 不让前端直接写 RustFS 或 Postgres。
- 不让前端直接连接 Python runtime、Redis Stream、MCP server、数据库或本地 shell。
- 不把前端 capabilities 当作最终权限；它只控制显示和入口。最终 allow/review/deny 永远以后端为准。
- 不在 UI 中保存 API key、MCP secret、数据库密码明文。桌面端只保存 FerrisKey token、设备标识和必要的本地 executor 配置。

## 4. 总体前端架构

```text
Tauri Desktop Shell
  src-tauri/
    commands/
      auth.rs
      secure_store.rs
      local_executor.rs
      system.rs
    events/
      auth_redirect
      local_exec
      app_lifecycle
    security/
      capability allowlist
      deep-link/loopback callback

React Renderer
  app shell
  feature domains
  XState machines
  TanStack Query cache
  Tauri event bus subscriber
  HTTP/SSE/WS adapters
        |
        v
Rust Backend Public API
  FerrisKey OIDC verified API
  Resource authz
  Run/event/approval/file/memory/workflow/catalog/audit
```

关键原则：

- Renderer 是展示层和用户交互层，不是权限边界。
- Tauri Rust side 是本机能力网关，不是业务权限事实源。本地执行、系统通知、文件选择、secure store 都要回到 Rust 后端授权语义。
- Backend adapter 只封装 public API；Internal API 不应被桌面端调用。
- 所有实时事件先归一化成前端 domain event，再分发给 XState actor 或 Query cache patch。
- i18n 只处理前端用户可见文案、状态 badge 和静态枚举标签；后端 DTO、错误码、状态机状态和权限语义保持原始协议值。
- 语言偏好可保存在 renderer 的非敏感本地存储；access token、refresh token、secret、设备凭证仍必须走 secure store 或受控 Tauri command。

## 5. 推荐技术栈

| 类别           | 推荐                                        | 说明                                                                                                                |
| -------------- | ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| Desktop shell  | Tauri 2                                     | 桌面端优先，Rust side 承接 deep link、secure store、本地 executor bridge 和系统事件。                               |
| UI             | React + TypeScript                          | 组件化与生态稳定，便于和 TanStack/XState/React Flow 组合。                                                          |
| Server state   | TanStack Query                              | 管理 HTTP 查询、缓存、重试、失效、乐观更新和分页。                                                                  |
| Process state  | XState                                      | 管理登录、run stream、审批、workflow editor/run、本地执行等长流程。                                                 |
| Event          | Tauri event bus + Browser EventTarget       | Tauri 系统事件进入 renderer；后端 SSE/WS 事件归一化后进入同一前端事件总线。                                         |
| DAG            | React Flow                                  | 工作流节点/边编辑、布局、选中、校验提示、运行态覆盖层。                                                             |
| Form           | React Hook Form + schema validation         | 表单多、schema 复杂，配合 Zod/Valibot 或生成 schema。                                                               |
| UI primitives  | Radix UI + lucide-react                     | 可访问性、弹窗、菜单、tabs、tooltip、select、switch；按钮内优先用图标。                                             |
| i18n           | 轻量 React Context + typed message resource | 当前只需要中英双语，先避免引入重型依赖；后续如需要复数、日期、命名空间懒加载或远程语言包，再评估 i18next/FormatJS。 |
| Virtualization | TanStack Virtual                            | 审计、事件、文件、记忆等长列表。                                                                                    |
| Code/editor    | Monaco 或 CodeMirror                        | 文件预览、JSON schema、workflow compiled plan、工具参数查看。                                                       |

不建议：

- 用 Zustand/Redux 保存后端事实数据。
- 在组件内直接写 `fetch('/api/v1/...')` 或 `invoke('...')`。
- 用单个全局 store 管理所有运行事件和所有页面状态。
- 把 SSE/WS message 直接渲染为 UI，不经过事件归一化与幂等处理。

## 6. Feature-based DDD 目录结构

建议在 `bibi_work_frontend` 下建立：

```text
bibi_work_frontend/
  package.json
  src/
    app/
      App.tsx
      routes.tsx
      providers.tsx
      app-shell/
      navigation/
      error-boundary/
    shared/
      api/
        http-client.ts
        sse-client.ts
        ws-client.ts
        query-client.ts
      tauri/
        invoke-client.ts
        event-bus.ts
        secure-store.ts
      ui/
      i18n/
        I18nProvider.tsx
        messages.ts
        index.ts
      icons/
      lib/
      types/
      contracts/
    features/
      auth/
      session-device/
      projects/
      conversations/
      runs/
      approvals/
      files/
      agents/
      skills/
      tools/
      mcp/
      llm/
      policies/
      memories/
      workflows/
      audit/
      local-exec/
      settings/
  src-tauri/
    Cargo.toml
    src/
      main.rs
      commands/
      events/
      secure_store/
      local_executor/
```

每个 feature 内部结构：

```text
features/runs/
  domain/
    run.types.ts
    run.events.ts
    run.machine.ts
    run.projections.ts
  api/
    run.adapter.ts
    run.queries.ts
    run.mutations.ts
    run.query-keys.ts
  components/
    RunTimeline.tsx
    RunStatusBadge.tsx
    ToolCallCard.tsx
    TaskList.tsx
    SubagentList.tsx
  screens/
    RunDetailScreen.tsx
  index.ts
```

依赖规则：

- `features/*/components` 可以依赖同 feature 的 `domain/api` 和 `shared/ui`。
- `features/*/api` 可以依赖 `shared/api`，不能依赖 React component。
- feature 之间只能通过公开 `index.ts` 暴露的类型、hook 或事件交互。
- `app/routes` 负责组合页面，不承载业务逻辑。
- `shared` 不允许反向依赖任何 feature。

## 7. Adapter layer 规范

### 7.1 HTTP adapter

所有 public API 调用集中到 adapter：

```ts
export interface RunApi {
  listRuns(params: ListRunsParams): Promise<Run[]>;
  getRun(runId: string): Promise<Run>;
  cancelRun(runId: string): Promise<void>;
  createRunStream(conversationId: string, input: CreateRunInput): Promise<Run>;
  listConversationEvents(
    conversationId: string,
    afterSeq?: number,
  ): Promise<RunEvent[]>;
}
```

实现要求：

- 自动注入 `Authorization: Bearer <FerrisKey access token>`。
- 401/403 统一抛出 `AuthExpiredError | ForbiddenError`。
- request/response 在 adapter 边界做 schema 校验和字段归一化。
- 后端 `snake_case` 可以在 adapter 内转为前端 domain 的 `camelCase`，禁止组件内混用两种风格。
- `tenant_id` 来自当前 session context，组件不手写。

### 7.2 Tauri invoke adapter

Renderer 不直接调用 `invoke`：

```ts
export interface DesktopAuthApi {
  openLoginUrl(url: string): Promise<void>;
  waitForOidcCallback(state: string): Promise<OidcCallback>;
  saveTokenSet(tokenSet: TokenSet): Promise<void>;
  loadTokenSet(): Promise<TokenSet | null>;
  clearTokenSet(): Promise<void>;
}
```

Tauri commands 按能力分组：

- `auth_open_external_browser`
- `auth_wait_callback`
- `secure_store_get`
- `secure_store_set`
- `secure_store_delete`
- `local_exec_register_device`
- `local_exec_start_bridge`
- `local_exec_stop_bridge`
- `system_get_device_info`
- `system_open_path`，仅用于打开后端允许的 artifact 或导出文件

所有 command 必须在 `tauri.conf.json` capabilities 中最小授权。

### 7.3 Event adapter

统一前端事件格式：

```ts
type AppEvent =
  | { type: "auth.callback"; payload: OidcCallback }
  | { type: "run.event"; payload: RunEvent }
  | { type: "approval.changed"; payload: ApprovalEvent }
  | { type: "file.changed"; payload: FileChangedEvent }
  | { type: "workflow.node.changed"; payload: WorkflowNodeEvent }
  | { type: "localExec.event"; payload: LocalExecEvent }
  | { type: "session.revoked"; payload: SessionRevokedEvent };
```

来源：

- Tauri event bus：OIDC callback、本地 executor、系统生命周期、通知点击。
- SSE/WS：conversation/run/workflow 实时事件。
- TanStack Query mutation success：需要通知 XState actor 的业务事件。

事件处理规则：

- 每个 `run.event` 以 `event_id` 或 `seq` 幂等去重。
- 后端事实先 patch Query cache，再通知相关 XState actor。
- 本地 executor 事件只能表达本机状态，不能直接把任务标记为平台 completed；平台 run/local_exec 状态以后端事件为准。

## 8. XState 状态机设计

### 8.1 App bootstrap machine

```text
booting
  -> loadingStoredToken
  -> unauthenticated
  -> refreshingToken
  -> loadingMe
  -> ready
  -> degraded
  -> fatal
```

职责：

- 读取 secure store token。
- 加载 `/api/v1/auth/oidc/config` 和 `/api/v1/me`。
- 创建/刷新 device projection。
- 捕获后端不可用、token 过期、session revoked。

不做：

- 不保存业务列表数据。
- 不处理具体页面的加载状态。

### 8.2 Auth OIDC machine

```text
idle
  -> preparingPkce
  -> openingBrowser
  -> waitingCallback
  -> exchangingCode
  -> savingToken
  -> loadingMe
  -> authenticated
  -> failed
```

桌面端推荐：

- 使用 Authorization Code + PKCE。
- callback 采用自定义协议 `bibi-work://auth/callback` 或 loopback `127.0.0.1`，由 Tauri Rust side 接收。
- `state`、`code_verifier` 只存在 secure store 或 Tauri Rust side 临时内存。
- access token 不进入普通 localStorage。

### 8.3 Run stream machine

```text
idle
  -> creatingRun
  -> connectingStream
  -> streaming
  -> waitingApproval
  -> waitingUserInput
  -> cancelling
  -> reconnecting
  -> completed
  -> failed
  -> cancelled
```

输入事件：

- `run.queued`
- `run.started`
- `message.delta`
- `message.completed`
- `tool.call.started/completed/failed`
- `task.created/updated/completed`
- `subagent.started/completed`
- `approval.requested/completed`
- `interrupt.requested`
- `run.completed/failed/cancelled`

职责：

- 管理当前 run 的流连接、断线重连、after_seq 回放。
- 把事件投射为 message、tool call、task、subagent、file changed、memory candidate。
- 控制发送框、停止按钮、审批提示和状态栏。

不做：

- 不直接查询 agent/tool/file 详情；这些属于 TanStack Query。
- 不决定审批 allow/deny；审批结果以后端 mutation 为准。

### 8.4 Approval machine

```text
idle
  -> loadingPending
  -> pending
  -> deciding
  -> decided
  -> conflict
  -> failed
```

职责：

- 展示待审批工具、风险、输入摘要、obligations、关联 run。
- 执行 `/api/v1/approvals/{approval_id}/decision`。
- 处理 run terminal 后审批 conflict。

审批 UI 必须显示：

- 工具名称、风险等级、资源、输入摘要。
- allow/review/deny 来源和 policy version。
- 可能影响：文件、MCP、SQL、本地命令、外部 HTTP。
- 审批后 evidence object reference 状态。

### 8.5 Workflow editor machine

```text
editing
  -> dirty
  -> validatingClient
  -> validatingServer
  -> valid
  -> invalid
  -> publishing
  -> published
```

职责：

- 本地校验空节点、缺 agent_version、缺字段、自环、环、悬空边。
- 调用后端 validate 固化服务端权限与 compiled plan 校验。
- 发布 workflow version。

Ordinus 的纯函数 DAG 校验可迁移到 `features/workflows/domain/workflow-graph.ts`。

### 8.6 Workflow run machine

```text
idle
  -> creating
  -> pollingDetail
  -> subscribing
  -> running
  -> waitingApproval
  -> cancelling
  -> completed
  -> failed
  -> cancelled
```

职责：

- 展示 workflow run 总状态、节点状态、依赖边、每个节点关联 agent run。
- 节点点击后打开对应 run timeline。
- 聚合上游失败、blocked/skipped、retry/backoff、timeout。

### 8.7 Local executor machine

```text
disabled
  -> registeringDevice
  -> idle
  -> connected
  -> requestReceived
  -> waitingApproval
  -> executing
  -> streamingOutput
  -> completed
  -> failed
  -> disconnected
```

职责：

- 管理桌面端本地执行 bridge 状态。
- 展示待执行请求、命令摘要、工作目录虚拟路径、风险、超时、输出上限。
- 严格等待 Rust 后端授权与审批结果。

禁止：

- renderer 自行拼接命令执行。
- 未经 Rust local exec request 的命令执行。
- 把本地绝对路径暴露给 Python 或作为 run input。

## 9. TanStack Query 设计

### 9.1 Query key 规范

```ts
["me"]["tenants"][("devices", tenantId)][("sessions", tenantId)][
  ("projects", tenantId, filters)
][("projectFiles", tenantId, projectId, prefix, pattern)][
  ("projectFile", tenantId, projectId, path, revisionOrVersion)
][("projectFileHistory", tenantId, projectId, path)][
  ("conversations", tenantId, filters)
][("conversationEvents", tenantId, conversationId, afterSeq)][
  ("runs", tenantId, filters)
][("run", tenantId, runId)][("approvals", tenantId, filters)][
  ("agents", tenantId, filters)
][("agent", tenantId, agentId)][("agentVersions", tenantId, agentId)][
  ("agentVersionCapabilities", tenantId, agentVersionId)
][("skills", tenantId, filters)][("tools", tenantId, filters)][
  ("mcpServers", tenantId, filters)
][("mcpTools", tenantId, mcpServerId)][("llmProviders", tenantId)][
  ("llmCredentials", tenantId, providerId?)][("llmModelProfiles", tenantId)
][("policyBindings", tenantId, resourceType, resourceId)][
  ("memories", tenantId, filters)
][("workflowDesigns", tenantId, filters)][
  ("workflowRun", tenantId, workflowRunId)
][("auditHashVerify", tenantId, limit)];
```

### 9.2 Mutation invalidation

| Mutation                             | Invalidate                                   |
| ------------------------------------ | -------------------------------------------- |
| create/update/disable agent          | `agents`, `agent`                            |
| publish agent version                | `agentVersions`, `agentVersionCapabilities`  |
| bind agent version                   | `agentVersionCapabilities`, `agentVersion`   |
| create/update skill/tool/MCP/LLM     | 对应 catalog 或 LLM keys                     |
| create policy binding                | `policyBindings`, 相关 resource capabilities |
| create conversation/run              | `conversations`, `runs`, `run`               |
| approval decision                    | `approvals`, `run`, `conversationEvents`     |
| memory activate/reject/archive/batch | `memories`, `memorySearch`                   |
| workflow design update               | `workflowDesigns`, `workflowDesign`          |
| workflow publish                     | `workflowVersions`, `workflowVersion`        |
| workflow run create/cancel           | `workflowRuns`, `workflowRun`                |
| revoke session/device                | `sessions/devices`, `me`                     |

实时事件 patch 原则：

- `run.event` append 到 conversation event cache。
- terminal event patch `run.status`。
- `file.changed` invalidate project file list/history/artifacts。
- `memory.candidate.created` invalidate memory candidate list。
- `workflow.node.*` patch workflow run detail node status。

## 10. 核心 UI 信息架构

### 10.1 应用一级导航

左侧导航建议：

1. 工作台
2. 项目
3. 智能体
4. 技能
5. 工具
6. MCP
7. LLM
8. 工作流
9. 记忆
10. 审批
11. 审计
12. 设置

管理员角色看到完整入口；普通用户仅看到其 capabilities 允许的入口。隐藏入口不是授权，所有操作仍以后端校验为准。

### 10.2 桌面工作台

布局：

```text
left sidebar           center workbench                 right inspector
------------------     ---------------------------      ---------------------
tenant/project         tab: agent chat                  run status
conversation list      tab: file viewer                 tasks
workflow runs          tab: workflow run                subagents
pending approvals      tab: artifacts                   approvals
                       bottom composer                  files changed
```

关键设计：

- 中间区以当前 conversation/run 为主，不做营销页。
- 右侧 inspector 可折叠分段：Run、Tasks、Subagents、Files、Approvals、Memory。
- run 中的 tool call 使用折叠卡片展示输入摘要、输出摘要、风险、耗时、审计关联，不默认展开敏感输出。
- tool call 展开后通过平台 renderer registry 渲染 Rust 校验后的 `ToolResultView[]`，支持表格、图表、地图、JSON、文件 diff 和 artifact；未知 view 降级为摘要/JSON 预览。
- task/subagent 使用状态组：pending、running、waiting、completed、failed/cancelled。
- 文件变更只显示虚拟路径、revision、hash、reason 和可读 diff/preview；不暴露本地真实路径。

### 10.2.1 工具结果渲染能力

前端不执行工具返回的任意 UI 代码。平台只渲染后端事件中通过 schema 校验的 `views`：

```ts
type ToolResultView =
  | TableToolResultView
  | ChartToolResultView
  | MapToolResultView
  | JsonToolResultView
  | FileDiffToolResultView
  | MarkdownToolResultView
  | ArtifactToolResultView;
```

推荐组件结构：

```text
features/runs/
  domain/
    tool-result-view.types.ts
    tool-result-view.schema.ts
  components/
    ToolCallCard.tsx
    tool-results/
      ToolResultRenderer.tsx
      TableResultView.tsx
      ChartResultView.tsx
      MapResultView.tsx
      JsonResultView.tsx
      FileDiffResultView.tsx
      ArtifactResultView.tsx
```

渲染规则：

- `ToolCallCard` 默认只显示摘要、风险、状态、耗时和审计/审批引用，避免时间线噪声。
- 展开后用 tabs 或分段显示 `Result / Input / Audit`；`Result` 内按 `view.kind` 调 registry。
- `table` 使用 TanStack Table，v1 支持 preview 行分页；带 `data_ref` 的大数据通过 artifact read API 按 500 行分页加载，并在前端做轻量虚拟滚动。
- `chart` v1 使用 Vega-Lite spec 真实渲染；禁止 JS formatter、HTML tooltip 和任意脚本。
- `map` v1 使用 MapLibre 渲染小型 GeoJSON `data_preview`；没有 preview 的 GeoJSON 通过 artifact read API 按需加载后渲染。
- `json` 支持折叠、复制、脱敏字段标识和大小限制。
- `file_diff` 只显示虚拟路径、revision、hash 和 diff/preview，不显示本地真实路径。
- `artifact` 只打开后端允许的对象引用；Tauri 打开外部文件仍必须走受控 command。
- renderer 失败不得影响 run timeline，fallback 到 `outputSummary` 和只读 JSON preview。

### 10.3 管理控制台

Agent 管理：

- agent draft 列表、详情、版本。
- system prompt、LLM profile、默认权限、记忆策略、风险策略。
- skill/tool/MCP/subagent binding。
- validate 和 effective-capabilities。
- 发布 immutable AgentVersion。

Skill 管理：

- skill draft、版本、内容摘要、注入模式、状态。
- 发布后只读。

Tool 管理：

- tool schema、版本、风险等级、执行器类型、状态。
- SQL/第三方工具只显示脱敏摘要和 schema/hash。

MCP 管理：

- server list、transport、has_config、has_secret_ref。
- discover tools、schema hash、tool visibility、disable。
- secret-backed server 在 resolver 未实现前显示 fail closed 状态。

LLM 管理：

- provider、model profile、credential 列表、创建、更新、禁用/撤销、参数策略。
- provider 详情来自通用 `ResourceResponse.metadata`；`created_at/updated_at` 必须是 RFC3339 string，前端 contract 不接受 Rust `OffsetDateTime` 默认数组格式。
- provider `base_url` 由用户配置并原样传给后端/runtime，前端不自动补 `/v1` 或改写版本路径。
- 不显示 `secret_ref` 原文。
- credential 只显示 `has_secret_ref`、owner、过期/撤销状态和 provider 关联；secret resolver 由后端运行时负责，前端不保存 secret value。

Policy 管理：

- resource type/id/action。
- subject user/role/relation。
- effect allow/deny/review。
- risk level、obligations、policy version、disabled_at。

### 10.4 记忆治理台

四层视图：

- core_profile
- episodic
- semantic
- procedural

筛选：

- status：candidate、active、rejected、archived。
- visibility：private、project、tenant、public。
- sensitivity：normal、sensitive、secret。
- user、agent、project、source_run。

操作：

- activate、reject、archive、batch decision。
- 搜索与相关 run 回溯。
- 显示 `untrusted` 标识和注入历史。

原则：

- 记忆内容永远是非可信上下文，不在 UI 中暗示它具有 system/developer 指令级别。
- candidate 默认需要治理，尤其是 `core_profile/procedural`。

### 10.5 Workflow DAG

编辑视图：

- React Flow canvas。
- 左侧节点库：v1 只有 agent task。
- 右侧属性面板：AgentVersion、instruction、expected output schema、input/output mapping、retry、timeout。
- 顶部 validate/publish/run。

运行视图：

- 同一 DAG 叠加运行状态。
- 节点色彩：pending、ready、queued、running、waiting_approval、completed、failed、blocked、cancelled。
- 点击节点展示对应 `workflow_node_run` 和 `agent_run_id` 的 run timeline。

校验：

- 前端本地快速校验：空节点、缺字段、缺 AgentVersion、自环、环、悬空边。
- 服务端 validate：权限、AgentVersion 状态、能力绑定、compiled plan。

## 11. 后端 API 到前端 feature 映射

| Feature            | API                                                                                                |
| ------------------ | -------------------------------------------------------------------------------------------------- |
| auth               | `GET /api/v1/auth/oidc/config`, `GET /api/v1/me`, `POST /api/v1/auth/logout`                       |
| session-device     | `/api/v1/devices`, `/api/v1/sessions`, revoke routes                                               |
| authz/policy       | `/api/v1/authz/check`, `/api/v1/authz/batch-check`, `/api/v1/policy-bindings`                      |
| catalog agents     | `/api/v1/agents`, `/api/v1/agent-versions/*`                                                       |
| catalog skills     | `/api/v1/skills`, `/api/v1/skill-versions/*`                                                       |
| catalog tools      | `/api/v1/tools`, `/api/v1/tool-versions/*`                                                         |
| catalog mcp        | `/api/v1/mcp-servers`, `/api/v1/mcp-tools/*`, discover                                             |
| catalog llm        | `/api/v1/llm-providers`, `/api/v1/llm-credentials`, `/api/v1/llm-credentials/{id}/revoke`, `/api/v1/llm-model-profiles` |
| projects/files     | `/api/v1/projects`, `/api/v1/projects/{id}/files*`, artifacts                                      |
| conversations/runs | `/api/v1/conversations`, `/api/v1/conversations/{id}/runs:stream`, events/SSE/WS, `/api/v1/runs/*` |
| approvals          | `/api/v1/approvals`, `/api/v1/approvals/{id}/decision`                                             |
| memories           | `/api/v1/memories*`                                                                                |
| workflows          | `/api/v1/workflow-designs`, `/api/v1/workflow-versions`, `/api/v1/workflow-runs`                   |
| audit              | `/api/v1/audit/hash-chain:verify`, `/api/v1/audit/hash-chain:seal`                                 |

## 12. 实时事件投射模型

后端标准事件到 UI：

| Event                            | UI projection                          |
| -------------------------------- | -------------------------------------- |
| `run.queued`                     | Run 状态 queued，timeline 新增         |
| `run.started`                    | 状态 running，启用停止按钮             |
| `message.delta`                  | assistant streaming buffer             |
| `message.completed`              | 固化 message                           |
| `tool.call.started`              | tool call card running                 |
| `tool.call.completed`            | card success，输出摘要，可选 ToolResultView 渲染 |
| `tool.call.failed`               | card error，错误摘要                   |
| `approval.requested`             | run machine -> waitingApproval，审批卡 |
| `approval.completed`             | approval cache patch，run 恢复中       |
| `interrupt.requested`            | inline HITL block                      |
| `task.created/updated/completed` | task panel                             |
| `subagent.started/completed`     | subagent panel                         |
| `file.changed`                   | file change list，invalidate files     |
| `memory.candidate.created`       | memory candidate badge                 |
| `workflow.node.*`                | DAG node status patch                  |
| `local_exec.*`                   | local exec panel                       |
| `run.completed/failed/cancelled` | terminal state，关闭 stream            |

前端归一化函数建议：

```ts
export function projectRunEvent(
  event: RunEvent,
  prev: RunProjection,
): RunProjection {
  switch (event.type) {
    case "message.delta":
      return appendDelta(prev, event);
    case "tool.call.started":
      return upsertToolCall(prev, event);
    case "task.updated":
      return upsertTask(prev, event);
    default:
      return prev;
  }
}
```

投射函数必须是纯函数，方便单元测试和事件回放测试。

`upsertToolCall` 应从 payload 中读取 `views` 并在 adapter/projection 边界用 schema 校验；校验失败的 view 不进入组件层。这样历史 `run_events` 可稳定回放，前端不会因为单个工具结果格式异常破坏整条时间线。

## 13. Tauri 桌面端安全设计

### 13.1 OIDC 与 token 存储

- 桌面端使用 FerrisKey public client。
- 使用 PKCE，不在前端保存 client secret。
- access token 和 refresh token 存 OS secure storage。
- renderer 只通过 Tauri command 获取短期内存 token，不直接读取持久化文件。
- token 刷新失败后进入 `unauthenticated`，清理 Query cache 和事件连接。

### 13.2 Device binding

启动后：

1. Tauri 获取设备名称、平台、稳定但不可逆的 device fingerprint。
2. React app 调用 `/api/v1/devices` 投影设备。
3. `/api/v1/me` 返回当前 device/session 摘要。
4. revoke 后关闭 SSE/WS/local executor bridge，并清理 token。

### 13.3 Local executor bridge

本地执行必须满足：

- Rust 后端创建 local exec request。
- Tauri bridge 只订阅分配给当前 device 的 request。
- 每个 request 含短期 token、run_id、project_id、command JSON、timeout、max_output_bytes、risk。
- 高风险命令需要审批完成后才执行。
- stdout/stderr 分块上报，前端只显示摘要和后端确认的事件。
- 任何路径必须通过虚拟路径映射和 containment 检查。

桌面端 UI：

- 顶部设备状态 badge：未启用、已连接、执行中、断开。
- Local Exec Center：pending/running/completed/failed request 列表。
- 高风险 request 的审批入口跳转到 approval detail。

## 14. 视觉与交互规范

整体风格：

- 企业级工作台，不做落地页。
- 高信息密度但保持分组清晰。
- 浅色中性背景，少量青绿色/蓝色作为状态或主操作，不做大面积紫蓝渐变。
- 8px 或更小圆角；卡片只用于列表项、工具卡、modal、重复资源项，不做卡片套卡片。
- 文本不使用 viewport width 缩放。
- 按钮优先使用 lucide 图标，复杂图标加 tooltip。

基础布局：

- 左栏：240-280px，可收起。
- 主工作区：最小宽度 520px。
- 右 inspector：300-420px，可折叠和分段 resize。
- 顶部工具栏：当前资源标题、状态、搜索、全局创建。
- 底部 composer：只在工作台会话页出现。

状态颜色：

- running：蓝色或青色。
- waiting/review：琥珀色。
- success：绿色。
- failed/deny：红色。
- archived/disabled：灰色。

可访问性：

- 所有图标按钮有 aria-label 或 tooltip。
- 审批/删除/禁用等破坏性动作使用确认 dialog。
- 表格、列表、DAG 节点支持键盘焦点。

## 15. 多阶段开发计划

### 阶段 0：前端工程骨架与契约层

目标：搭建可扩展工程骨架，固定 adapter、feature、状态和测试边界。

工作项：

1. 初始化 Tauri + React + TypeScript 工程。
2. 配置 ESLint、Prettier、Vitest、Testing Library、Playwright。
3. 建立 `shared/api`、`shared/tauri`、`features/*` 目录。
4. 实现 HTTP client、token provider、error model、query client。
5. 手写或生成当前后端 DTO contracts。
6. 实现基础 design system：Button、Input、Dialog、Tabs、Table、Badge、Tooltip、Resizable panels。

验证：

- `npm run typecheck`、`npm run test` 通过。
- adapter contract 测试能 mock `/api/v1/me`、`/api/v1/conversations`。
- 组件内搜索不到直接 `fetch(` 和直接 `invoke(`。

### 阶段 1：OIDC 登录、App Shell、session/device

目标：完成可登录、可退出、可识别当前用户与设备的桌面壳。

工作项：

1. 实现 OIDC config 获取与 PKCE 登录 machine。
2. Tauri deep link/loopback callback command。
3. secure store token 保存、读取、清理。
4. `/api/v1/me` 当前用户上下文。
5. session/device 列表、revoke、logout。
6. App Shell、导航、capabilities-based visibility。

验证：

- 无 token 启动进入登录页。
- 登录后能进入工作台并显示 user/tenant/roles/device/session。
- revoke 当前 session 后自动回到登录态并清空 cache。
- 普通用户看不到管理员入口，但直接访问管理路由仍会被后端 403 拦截。

### 阶段 2：工作台、运行流、task/subagent、审批闭环

目标：形成智能体运行平台的核心日常工作流。

工作项：

1. Conversation list/create。
2. Run create via `/conversations/{id}/runs:stream`。
3. SSE 订阅和 after_seq replay。
4. Run stream XState machine。
5. Message timeline、tool call card、task panel、subagent panel。
6. Approval center 和 inline approval card。
7. Run cancel。

验证：

- 创建 run 后能看到 `run.started/message.delta/message.completed/run.completed`。
- `task.*` 更新右侧 task panel。
- `subagent.*` 更新右侧 subagent panel。
- `approval.requested` 进入 waitingApproval，批准/拒绝后 UI 与后端状态一致。
- SSE 断线后能按 after_seq 恢复且不重复渲染事件。

### 阶段 3：项目与文件只读工作区

目标：提供企业项目文件浏览、搜索、历史和产物查看。

工作项：

1. Project list/create、mount list/create。
2. File tree 使用 `entries` 构造虚拟目录。
3. File read 支持 latest/revision/version_id。
4. File search、history、artifacts。
5. 文件 tabs：文本、代码、Markdown、图片、PDF、二进制 metadata。
6. run 中 `file.changed` 与文件树联动刷新。

验证：

- 路径显示始终是 `/workspace/...` 等虚拟路径。
- 二进制默认不渲染内容，只显示 metadata，显式 allow_binary 才展示可支持预览。
- 历史 revision 能打开只读视图。
- public API 不提供写入入口时，UI 不显示直接保存按钮。

### 阶段 4：Catalog 管理与策略绑定

目标：管理员可以创建、配置、发布和授权智能体能力。

工作项：

1. Agent/AgentVersion 管理。
2. Skill/Tool/MCP/LLM 管理。
3. AgentVersion binding editor。
4. Effective capabilities 和 validate 视图。
5. Policy binding editor。
6. MCP discover 和 tool schema/hash 展示。

验证：

- alon 能创建 agent、发布版本、绑定 skill/tool/MCP。
- alice 访问管理接口显示 403 状态和只读/无权说明。
- AgentVersion 已被 run 使用后，绑定编辑 UI 显示不可变约束。
- secret 字段只显示 `has_secret_ref`，不显示原值。

### 阶段 5：记忆治理台

目标：用户和管理员可治理四层记忆，避免记忆污染与越权可见。

工作项：

1. Memory list/search/filter。
2. Candidate inbox。
3. activate/reject/archive/batch-decision。
4. memory source run link。
5. untrusted/sensitivity/visibility 标识。

验证：

- candidate 到 active/rejected/archive 状态转换正确。
- 不同 layer/status/visibility 过滤正确。
- secret/sensitive 记忆有明确视觉标识，不在普通摘要中泄露。

### 阶段 6：Workflow DAG 设计与运行可视化

目标：支持专家智能体 DAG 编排、发布、运行和节点级观测。

工作项：

1. Workflow design list/detail/create/update。
2. React Flow DAG editor。
3. 本地 DAG 校验和服务端 validate。
4. publish workflow version。
5. create workflow run。
6. workflow run DAG 状态 overlay。
7. node run -> agent run timeline drill-down。

验证：

- 环、自环、悬空边、缺 agent_version、缺字段在前端被拦截。
- 服务端 validate 错误能映射到节点/边/全局错误。
- 3 节点 DAG 能展示依赖顺序、并行节点、失败 blocked/skipped。
- cancel workflow run 后 UI 和后端状态一致。

### 阶段 7：本地执行桌面 bridge

目标：实现桌面端受控本地执行入口，支撑“智能体在本地直接执行任务”。

工作项：

1. Tauri local executor command 和事件桥。
2. Device-bound bridge 连接状态。
3. local exec request 列表、详情、输出流。
4. 高风险命令审批联动。
5. stdout/stderr 截断与脱敏展示。
6. 本地路径 containment 与虚拟路径映射提示。

验证：

- 未经后端 request 的命令不能执行。
- 高风险 request 未审批时不执行。
- request 输出超过上限后 UI 显示截断状态。
- device revoke 后 bridge 断开并拒绝新 request。

### 阶段 8：审计、稳定性和企业化收尾

目标：补齐审计视图、性能、错误恢复和桌面发布质量。

工作项：

1. Audit hash-chain verify/seal UI。
2. 审批 evidence、tool-call evidence、artifact 引用展示。
3. 全局错误中心和诊断导出。
4. 大列表 virtualization。
5. SSE/WS reconnect 压测。
6. Tauri updater、签名、配置环境切换。

验证：

- audit_admin 可执行 verify/seal，普通用户无权。
- 10k 事件回放不卡死。
- 后端重启后前端能恢复连接或清晰提示。
- 打包产物通过基础安装/启动/登录/运行 smoke test。

## 16. 测试策略

单元测试：

- XState machine transition。
- event projection pure functions。
- query key factory。
- adapter response normalization。
- i18n message key parity、插值和默认语言 fallback。
- DAG cycle detection、node validation。
- path display normalization。

组件测试：

- Run timeline、ToolCallCard、TaskPanel、SubagentPanel。
- ApprovalDialog。
- FileTree/FileViewer。
- AgentVersionBindingEditor。
- MemoryGovernanceTable。
- WorkflowCanvas。
- 语言切换后 App Shell、导航、工作台、审批、项目文件和记忆治理台关键文案同步变化。

集成测试：

- MSW/mock backend 覆盖 auth、run events、approval、file、workflow。
- Tauri invoke mock 覆盖 secure store、OIDC callback、local executor events。
- TanStack Query invalidation 行为。

桌面 E2E：

- 首次启动登录。
- 创建 conversation/run，接收流式事件。
- 审批高风险工具后 run 继续。
- 浏览项目文件和历史。
- 创建并发布 workflow，运行后查看节点状态。
- revoke session/device 后自动收敛。

契约测试：

- 从 `bibi_work_backend/src/features/agent_platform/models.rs` 对齐核心 DTO。
- 后续后端补 OpenAPI/TS schema 后，前端改为生成 contracts，并保留 adapter 层领域映射。

## 17. 关键风险与应对

| 风险                          | 应对                                                                                                |
| ----------------------------- | --------------------------------------------------------------------------------------------------- |
| 前端绕过 adapter 直接调用 API | lint rule + code review + adapter contract 测试。                                                   |
| server state 和 UI state 混乱 | TanStack Query 只放服务端事实，XState 只放流程状态，本地 UI 折叠/选中放 component/local store。     |
| run events 重复或乱序         | 使用 `seq/event_id` 幂等，按 Postgres seq 投射，SSE reconnect 使用 after_seq。                      |
| 权限入口与真实权限不一致      | capabilities 只用于显示，mutation 失败必须展示后端 reason，不在前端预判 allow。                     |
| OIDC token 泄露               | secure store，renderer 不持久化 token，不打日志。                                                   |
| 本地执行越权                  | 本地 executor 只能消费 Rust request，执行前校验短期 token、device、approval、timeout、path policy。 |
| Workflow 前后端校验不一致     | 前端做快速 UX 校验，发布前必须后端 validate，错误映射回 UI。                                        |
| 文件编辑能力误导用户          | public file API 只读阶段不显示直接保存；编辑通过 agent task 或后续受控写 API。                      |

## 18. 后续实现核心原则

第 18 章之后不再保留逐轮流水账，只维护当前事实和后续优先级。

从资深项目架构师视角：

- 前端必须继续围绕“任务创建 -> 运行事件 -> 工具/文件/审批 -> 记忆沉淀 -> 管理治理”的主闭环推进。
- 桌面端不是新的事实源。项目、运行、审批、记忆、Catalog、Policy、审计都以后端 Rust public API 和 Postgres/RustFS 事实为准。
- Workflow、Audit、local executor 都是扩大风险面的模块，应在 OIDC 与真实 run E2E 稳定后再做。

从 Rust 专家视角：

- Renderer 不直接访问 Python、Redis、RustFS、MCP server、数据库或本地 shell。
- 前端 adapter 只调用 Rust 后端 public API；涉及高风险写入、审批、审计链和本地执行的能力必须回到后端状态机。
- 记忆治理已按后端事实返回 `source_run_id`，前端只展示和投影，不伪造 run 关联。

从 Tauri 专家视角：

- Tauri side 只保留本机能力网关：打开外部浏览器、secure store、设备信息和本地 executor bridge 状态。
- 当前 `local_exec_*` command 明确返回 `disabled`，没有 renderer 直接执行命令的入口。
- 后续 OIDC 必须补齐 deep link 或 loopback callback、token exchange、refresh 和 revoke 后全局收敛。

从 React 设计专家视角：

- 继续保持 feature-based DDD：组件不直接 `fetch` 或 `invoke`，只使用 adapter/query/domain projection。
- TanStack Query 管 server state；运行事件统一进入 Query cache 后再投射 timeline/task/subagent/file/approval。
- XState 只处理长流程状态，不作为后端事实缓存。
- UI 维持工作型三栏、高密度、低噪声风格；新增页面必须接入 `shared/i18n` 的中英文 message resource。

## 19. 最新代码实现状态

更新时间：2026-06-22。依据当前 `bibi_work_frontend` 与相关后端代码。

| 模块             | 当前状态                  | 说明                                                                                                                                                                                                                                                                                                                                      |
| ---------------- | ------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 工程骨架         | 已实现                    | `Tauri 2 + React + TypeScript + Vite`，含 ESLint、Prettier、Vitest、Tauri scripts。                                                                                                                                                                                                                                                       |
| 架构边界         | 已实现基础约束            | `shared/api`、`shared/tauri`、`shared/contracts`、`features/*` 已落地；有架构测试禁止组件直接 `fetch/invoke`。                                                                                                                                                                                                                            |
| App Shell 与导航 | 已实现                    | 左侧 feature 导航、顶部用户/语言/退出、capability/admin 可见性控制。                                                                                                                                                                                                                                                                      |
| i18n             | 已实现当前范围            | `zh-CN` 与 `en-US` typed message resource 已接入主要页面；后端错误和 agent/tool 自然语言内容不翻译。                                                                                                                                                                                                                                      |
| 登录与会话       | 部分实现                  | 已有 OIDC config、PKCE URL、外部浏览器打开、token secure-store/browser-dev 存取、`/me`、logout、401 清理 Query cache；未完成 callback、token exchange、refresh、session/device revoke 后主动断流。                                                                                                                                        |
| Tauri commands   | 已实现最小安全集          | `auth_open_external_browser`、`secure_store_get/set/delete`、`system_get_device_info` 已实现；本地执行 command 仍为 disabled stub。                                                                                                                                                                                                       |
| 工作台与运行事件 | 前端闭环已实现            | conversation/run/approval/project adapters、三栏工作台、run timeline、tool/task/subagent/file/approval projection 已实现；会话事件 SSE 支持 `after_seq` 恢复、去重合并和退避重连；工具结果已补 `ToolResultView` schema、projection、renderer registry、TanStack Table 分页、Vega-Lite chart、MapLibre GeoJSON preview、artifact 按需读取分页和表格虚拟滚动。                                                                                                                                                        |
| 审批             | 已实现基础 UI             | 审批中心、compact approval、decision mutation 已接入；仍需真实后端运行链路 E2E 验证。                                                                                                                                                                                                                                                     |
| 项目文件         | 已实现只读工作区          | 支持 project list、文件 list/read/search/history/artifacts、revision/version 读取、二进制显式加载、`file.changed` query invalidation；没有保存入口。                                                                                                                                                                                      |
| 记忆治理         | 已实现主界面              | 支持 list/search/create、activate/reject/archive、batch decision、layer/status/query 筛选、source run 展示；visibility/sensitivity 仍主要由前端二次过滤。                                                                                                                                                                                 |
| Catalog/Policy   | 已实现阶段 4 主要管理闭环 | Agent/Skill/Tool 列表与详情、创建、资源禁用、版本发布、版本禁用已接入；Policy binding 支持按当前资源创建与禁用；AgentVersion 支持 effective-capabilities 查看和 validate 结果展示；MCP 列表与 MCP tools 仍是只读；metadata 做前端递归脱敏；AgentVersion binding、MCP discover/tool publish 尚未实现。 |
| LLM 管理         | 已实现正式管理页并完成一次真实配置验证 | 独立 `features/llm` adapter/query/page 已接入，支持 Provider/Profile/Credential 列表、Provider 创建/更新/禁用、Credential 创建/撤销、Profile 创建/更新/禁用；Credential 详情只显示 `has_secret_ref`、owner、过期/撤销状态，不显示 `secret_ref` 原文；Provider/Profile/Credential 均按 `ResourceResponse.metadata` 映射，时间字段按 RFC3339 string 校验；OpenAI-compatible provider 的 `base_url` 原样保存和透传。 |
| Workflow DAG     | 未实现                    | 导航入口存在，当前仍是 placeholder。                                                                                                                                                                                                                                                                                                      |
| Audit            | 未实现                    | 导航入口存在，当前仍是 placeholder。                                                                                                                                                                                                                                                                                                      |
| Settings         | 未实现                    | 当前仍是 placeholder。                                                                                                                                                                                                                                                                                                                    |
| Local executor   | 未实现执行能力            | Tauri/Rust 侧只返回 disabled 状态；不注册设备、不拉取 request、不执行命令。                                                                                                                                                                                                                                                               |

## 20. 已验证状态

最近记录的验证结果：

- `bibi_work_frontend`：使用 `fnm exec --using v24.13.0` 执行 `npm run typecheck`、`npm run lint`、`npm run test`、`npm run build` 通过。
- Vitest：9 个测试文件、18 个测试用例通过，新增覆盖 Policy binding 写入/禁用、AgentVersion effective-capabilities 和 validate adapter 契约。
- 本轮 LLM 管理补充验证：`npm run typecheck`、`npm run lint`、`npm test -- src/features/llm/api/llm.adapter.test.ts src/test/architecture-boundaries.test.ts` 通过。
- 本轮 LLM Provider contract 修复后验证：`npm run test -- llm.adapter.test.ts` 通过；真实 API 查询确认 `/api/v1/llm-providers` 返回的 `created_at/updated_at` 为 string，credential 响应只暴露 `has_secret_ref`，不暴露 `secret_ref` 或 API key。
- `bibi_work_frontend/src-tauri`：`cargo fmt`、`cargo check` 已在前序实现中通过；最新 LLM 管理页改动没有改 Tauri Rust 代码。
- `bibi_work_backend` / `bibi_work_agent`：LLM credential 与 env-backed runtime credential resolver 已通过 `cargo check`、`cargo test secret_resolver`、`uv run pytest tests/test_agent_factory.py`、`uv run ruff check bibi_work_agent/clients/rust_client.py bibi_work_agent/runtime/agent_factory.py tests/test_agent_factory.py`。
- 本轮 openai-compatible smoke：已配置 `openai-compatible` provider、`minimax-m2.5` profile 和 env-backed credential；Rust 后端 dispatch 到 Agent API、Python worker 获取短期 runtime credential 后真实调用兼容模型服务，run 状态为 `completed`。

后续涉及后端契约或 Tauri command 改动时，应按影响面选择最小验证命令。

## 21. 当前未完成事项与下一步

必须优先补齐：

1. OIDC 完整登录：deep link/loopback callback、token endpoint exchange、refresh token、revoke/401/403 后关闭 SSE 并清空 Query cache。
2. 真实 run E2E 扩展：LLM Provider 到 Python worker 的最小真实 run 已验证；仍需覆盖 conversation create、approval decision、run cancel、SSE replay 去重和断线恢复。
3. 工具结果渲染深化：`ToolResultView` schema、projection、renderer registry、TanStack Table、Vega-Lite、MapLibre preview、artifact 内容按需读取分页和大表虚拟滚动已接入；下一步补真实跨进程 run 回放 E2E 和对象存储 range/streaming reader。
4. Catalog 剩余写入闭环：AgentVersion binding editor、MCP discover/tool publish。LLM Provider/Profile/Credential 管理页已接入，后续主要补 credential rotate、model profile test、fallback 策略 UI 和真实 run E2E。

暂缓事项：

- Workflow DAG：等 run/approval 真实 E2E 稳定后再实现。
- Audit UI：等核心写入与审批链路稳定后再做查询、verify/seal 操作面。
- Local executor bridge：等审批、审计和 device-bound request 协议稳定后再实现；在此之前必须保持 fail closed。

bibi: 压缩第 18 章及后续章节是为了删除重复的逐轮执行记录，保留当前事实和下一步决策依据；局限是丢失历史细节；如以后需要审计开发过程，可单独维护变更日志。
