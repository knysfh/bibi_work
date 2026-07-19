# Agent MCP 细粒度能力与版本安全改造

## 1. 目标

本轮改造解决四个直接问题：

1. Agent 可以按 MCP Tool 选择能力，不再因为选择一个 MCP Server 而接受其全部工具。
2. AgentVersion 固定发布时审核过的 Tool ID 与 schema hash，MCP 重新发现后不能静默改变已发布能力。
3. 会话只能在 AgentVersion 允许的工具范围内选择子集，不能通过会话 MCP 配置扩权。
4. 当管理员从新版本中移除能力时，旧版本自动撤销；已有会话保留历史，但下一次 Run 自动迁移到最新安全版本。

## 2. 分层职责

### Agent

Agent 是运行能力与安全上限，负责：

- Runtime、模型、系统提示词和行为策略。
- Skill、Tool、SQL Tool 与 MCP Tool allowlist。
- 每个 Tool 发布时的 schema hash。
- 版本发布、校验和撤销。

### Assistant

Assistant 是用户可见预设，负责：

- 名称、头像、描述和推荐提示词。
- 默认模型、权限模式和默认 MCP Server。
- 默认值只能在 Agent 允许范围内取子集，不能给 Agent 扩权。

### Conversation

Conversation 保存历史与临时偏好：

- 会话可选择 MCP Server，但最终只启用该 Server 中已被 AgentVersion 允许的 Tool。
- 会话不拥有永久安全权限。
- 每次 Run 保存实际使用的 AgentVersion 和不可变运行快照。

## 3. 有效工具计算

本轮将原有并集：

```text
AgentVersionTools + ConversationServerTools
```

调整为限制关系：

```text
ActiveTools = AgentVersionAllowedTools
              ∩ ConversationSelectedServers
              ∩ ActiveServerAndTool
```

规则：

- 会话未选择任何 MCP Server 时，使用 AgentVersion 中全部绑定工具作为默认能力。
- 会话选择 Server 时，仅保留绑定工具中属于这些 Server 的部分。
- AgentVersion 未绑定的 Tool 永远不会因为会话选择 Server 而进入运行快照。
- Tool 或 Server 被实时禁用后，新 Run 不再加载；执行阶段也必须再次检查 active 状态与用户授权。

## 4. AgentVersion 与 schema 固定

`agent_version_mcp_bindings` 除 `agent_version_id`、`mcp_tool_id` 外保存：

```text
schema_hash_at_publish
binding_mode: required | optional
```

本轮 UI 产生的绑定使用 `optional`，会话可以关闭，但不能新增未绑定工具。

运行时只加载 schema hash 仍与发布时一致的 Tool。重新发现导致 schema 变化时：

- 有效能力查询把该绑定标记为 stale。
- AgentVersion 校验失败。
- Run 不加载该工具。
- 管理员需要重新检查工具并发布新版本。

## 5. 发布与旧会话迁移

Agent 配置保存会创建新的 immutable AgentVersion，并复制非 MCP 能力绑定。

### 仅增加能力

- 新版本成为新会话默认版本。
- 旧版本继续 published，旧会话保持原行为。
- 旧会话不会自动获得新增权限。

### 移除能力或修复 stale schema

- 新版本发布成功后自动 disable 旧版本。
- 已有会话下一次创建 Run 时发现 pinned version 不再 published。
- 后端解析同一 Agent 的最新 published 版本，更新会话 pin，并用新版本创建 Run。
- 每个历史 Run 仍保留原 `agent_version_id`，审计链不被改写。

### 紧急撤销

管理员应优先禁用具体 MCP Tool/Server 或增加执行时 Deny Policy。仅禁用 AgentVersion 只能阻止后续 Run，不能撤销已经提交到外部系统的副作用。

### 思考模型的多轮 Tool Call 兼容

部分 OpenAI-compatible 思考模型会在 Tool Call 响应中返回 `reasoning_content`，并要求下一轮把该字段随原 Assistant 消息回放。通用 OpenAI 客户端默认会丢弃该扩展字段，导致同一会话第二轮返回 HTTP 400。

运行时适配器必须：

- 在流式和非流式响应中保留 `reasoning_content` 到内部 checkpoint。
- 重放历史 Assistant Tool Call 消息时把该字段原样传给模型服务。
- 不把隐藏推理内容写入用户可见的消息事件；前端仍只显示经过过滤的最终内容。

## 6. Agent 配置界面

Agent 详情页增加 MCP Tools 卡片：

- 按 MCP Server 分组。
- 展示 Tool 名称、描述和风险标签。
- 支持单 Tool 复选、多选、全选只读工具和清空。
- 展示已选择数量和 schema 变化警告。
- 保存按钮发布新 AgentVersion，不直接修改已经执行过的版本。
- Tool 列表区域限制高度并内部滚动，避免页面与弹窗溢出。

风险信息来自平台治理事实；MCP Server annotation 只作为提示，不能作为唯一安全依据。

## 7. 权限与市场

市场负责发现和分发，不是最终安全边界。后续应拆分：

```text
agent:discover | use | manage | publish
mcp_server:discover | use | manage
mcp_tool:bind | execute | approve_high_risk
```

目录 API 需要按用户权限过滤可见资源；但即使 Tool 在市场中不可见，执行 API 仍必须实时鉴权，以阻止旧会话、缓存或直接 ID 调用绕过界面限制。

## 8. 测试验收

### Rust

- AgentVersion 绑定保存 schema hash。
- stale schema 不进入运行快照并导致版本校验失败。
- 会话选择 MCP Server 只能过滤 Agent 已绑定工具。
- 移除 Tool 发布新版本后旧版本被禁用。
- pinned 旧版本被禁用后，下一次 Run 迁移到最新 published 版本。
- Tool/Server 禁用后执行 fail closed。

### Frontend

- Agent MCP Tool 列表按 Server 分组。
- 单 Tool 选择、全选只读和清空正确。
- 保存 payload 只包含选中的 Tool ID。
- loading、empty、error、stale 状态可见。
- 小窗口下无横向溢出，长列表内部滚动。

### Live E2E

- 为测试 Agent 绑定 `maps_geocode`，会话选择 Google Maps Server 后可以调用。
- 同 Server 未绑定 Tool 不出现在有效运行快照。
- 从 Agent 中移除 Tool 后旧版本被撤销，已有会话下一轮不能再调用。
- 思考模型完成 Tool Call 后可继续同一会话，且撤权迁移后不会因缺少 `reasoning_content` 报错。
- Settings Agent 页面截图无溢出、遮挡或异常状态。

## 9. 后续阶段

1. Agent/MCP 市场的资源级可见性、安装、绑定和执行权限。
2. `draft -> review -> published -> deprecated -> revoked` 完整发布工作流。
3. 发布差异审查，突出新增写入、删除和 schema 变化。
4. required/optional Tool、逐 Tool approval mode 和管理员风险覆写。
5. Tool 数量超过上下文阈值时使用 progressive discovery，只把任务相关 schema 注入模型。
6. 安全发布的 Run 取消、待审批调用失效和受影响会话通知。
