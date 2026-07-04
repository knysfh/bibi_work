# 本地服务配置

本文档记录本地开发环境中 FerrisKey 和 RustFS 的连接信息、初始化状态和使用约定。只有任务需要直接操作认证授权或对象存储时才需要读取。

## FerrisKey

- Web 端地址：`http://localhost:5555`
- API 地址：`http://localhost:3333`
- 管理员账号：`admin`
- 管理员密码：`admin`
- 已创建 realm：`bibi-work`
- OIDC discovery：`http://localhost:3333/realms/bibi-work/.well-known/openid-configuration`
- master realm token endpoint：`http://localhost:3333/realms/master/protocol/openid-connect/token`
- bibi-work realm token endpoint：`http://localhost:3333/realms/bibi-work/protocol/openid-connect/token`

注意：`5555` 是 FerrisKey Web 前端端口，`3333` 是 FerrisKey API/OIDC 端口。不要把后端服务的 OIDC 或 token 请求发到 `5555`。

### 建议 client 规划

- `bibi-work-desktop`：桌面端 public client，使用 Authorization Code + PKCE。
- `bibi-work-web`：Web 端 public client，使用 Authorization Code + PKCE。
- `bibi-work-backend`：Rust 后端 client，用于 token 校验、introspection 或服务端回调。本地 smoke test 会临时开启 password grant，生产环境应关闭用户密码直换 token，改走前端/桌面端 Authorization Code + PKCE。
- `bibi-work-runtime`：Python agent runtime/internal confidential client，用于运行态服务间认证。

### 建议 token scope/claim

- scope：`openid profile email roles`
- roles claim：优先使用 `realm_access.roles`，或单独映射为 `roles`
- audience：建议包含 `bibi-work-backend`。当前本地 FerrisKey token 会把调用方 client 放在 `azp`，Rust 后端已兼容 `aud` 或 `azp` 等于 `bibi-work-backend`。

### 本地 bootstrap 与 token 获取

本地初始化使用仓库脚本同步 FerrisKey realm/client/role/user 与 Postgres 投影：

```bash
DATABASE_URL=postgresql://postgres:password@127.0.0.1:5433/bibi_work \
FERRISKEY_BASE_URL=http://localhost:3333 \
FERRISKEY_ADMIN_USERNAME=admin \
FERRISKEY_ADMIN_PASSWORD=admin \
FERRISKEY_ALON_PASSWORD='BFD@123' \
FERRISKEY_ALICE_PASSWORD='BFD@123' \
python3 bibi_work_backend/scripts/bootstrap_ferriskey.py
```

本地命令行 smoke test 可以用 `bibi-work-backend` client 的 password grant 获取 token：

```bash
ALON_TOKEN=$(curl -sS -X POST http://localhost:3333/realms/bibi-work/protocol/openid-connect/token \
  -H 'content-type: application/x-www-form-urlencoded' \
  --data-urlencode grant_type=password \
  --data-urlencode client_id=bibi-work-backend \
  --data-urlencode username=alon \
  --data-urlencode password='BFD@123' \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["access_token"])')

ALICE_TOKEN=$(curl -sS -X POST http://localhost:3333/realms/bibi-work/protocol/openid-connect/token \
  -H 'content-type: application/x-www-form-urlencoded' \
  --data-urlencode grant_type=password \
  --data-urlencode client_id=bibi-work-backend \
  --data-urlencode username=alice \
  --data-urlencode password='BFD@123' \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["access_token"])')
```

调用 Rust protected API：

```bash
curl -sS http://127.0.0.1:8361/api/v1/tenants \
  -H "authorization: Bearer ${ALON_TOKEN}"
```

如果 `8361` 已被旧后端实例占用，可临时换端口运行当前代码：

```bash
cd bibi_work_backend
APP_APPLICATION__PORT=18361 APP_INTERNAL__SHARED_TOKEN=local-internal-token cargo run
```

再用 `http://127.0.0.1:18361/api/v1/tenants` 验证。

当前已确认 FerrisKey API 中 `alon/alice` 用户和角色存在；但本地签发的 access token 里 `realm_access.roles` 仍为空。Rust 端已能通过 Postgres tenant membership/role projection 完成当前资源授权 smoke test；如果后续策略必须完全依赖 token roles claim，需要继续调整 FerrisKey client scope/protocol mapper。

### 项目授权检查注意事项

当前项目代码中的 FerrisKey 授权检查路径配置为：

```yaml
ferriskey:
  check_path: /api/v1/authz/check
  batch_check_path: /api/v1/authz/batch-check
```

FerrisKey 本地 API 当前未确认提供上述资源级 PDP 端点。现阶段应先用 FerrisKey/OIDC 做身份认证和粗粒度 roles claim，再由 Rust 后端实现或适配资源级 `authz/check`，用于 agent/tool/skill/MCP/file/local-exec 的二次鉴权。

## RustFS

- S3/API endpoint：`http://localhost:9004`
- 兼容 S3 后端端口：`http://localhost:9003`
- Access key：`rustfsadmin`
- Secret key：`rustfsadmin`
- Region：`us-east-1`

### 已初始化 bucket

- `bibi-work-marketplace`
- `bibi-work-files`
- `bibi-work-runs`
- `bibi-work-memory`
- `bibi-work-audit`

初始化状态：

- 以上 bucket 均已创建。
- 以上 bucket 均已启用 versioning。
- 已写入目录占位对象和 `tenants/bibi-work/_manifest.json`。

### 目录规划摘要

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

个人目录不要用可变用户名，创建真实用户后使用不可变 `user_id`：

```text
bibi-work-files/tenants/bibi-work/users/{user_id}/remote-space/
bibi-work-memory/tenants/bibi-work/users/{user_id}/core-profile/
bibi-work-memory/tenants/bibi-work/users/{user_id}/episodic/
bibi-work-memory/tenants/bibi-work/users/{user_id}/semantic/
bibi-work-memory/tenants/bibi-work/users/{user_id}/procedural/
```
