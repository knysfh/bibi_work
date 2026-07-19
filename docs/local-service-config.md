# 本地服务配置说明

本项目把本地联调配置分为两层：

- `docs/local-service-config.local.md`：给开发者阅读，记录本机真实服务、账号与初始化说明。
- `bibi_work_backend/.env.local`：给进程读取，保存本机 E2E、冒烟和集成测试变量。

两者均为本机文件并由 Git 忽略。不得提交，也不得把其中的值写入日志、测试快照、Issue 或回复。本文档只定义结构，不保存真实地址、账号、密码或密钥。

## 配置优先级

`services.sh` 按以下顺序加载配置，后加载的值覆盖先加载的值：

1. 根目录 `.env`、`.env.local`
2. `bibi_work_backend/.env`、`bibi_work_backend/.env.local`
3. `bibi_work_agent/.env`、`bibi_work_agent/.env.local`
4. `bibi_work_frontend/.env`、`bibi_work_frontend/.env.local`

当前项目的本机测试变量统一维护在 `bibi_work_backend/.env.local`。独立执行测试前应显式导入：

```bash
set -a
. bibi_work_backend/.env
. bibi_work_backend/.env.local
set +a
```

不要从 Markdown 中用 `grep`、`awk` 等方式抽取凭证；Markdown 是人工说明，`.env.local` 才是进程配置源。

## 必需配置结构

### 数据库与运行时

```dotenv
DATABASE_URL=<postgres-url>
APP_INTERNAL__SHARED_TOKEN=<local-shared-token>
APP_AGENT_RUNTIME__SHARED_TOKEN=<same-local-shared-token>
APP_AGENT_RUNTIME__BASE_URL=<agent-api-url>
BIBI_AGENT__INTERNAL_TOKEN=<same-local-shared-token>
BIBI_AGENT__RUST_BASE_URL=<rust-backend-url>
BIBI_AGENT__DATABASE_URL=<postgres-url>
BIBI_AGENT__CELERY_BROKER_URL=<redis-broker-url>
BIBI_AGENT__CELERY_RESULT_BACKEND=<redis-result-url>
BIBI_WORK_INTERNAL_TOKEN=<same-local-shared-token>
```

三个内部 token 必须一致，否则 Rust 后端、Agent API 和 Celery worker 会互相拒绝请求。

### FerrisKey 与桌面 E2E

```dotenv
FERRISKEY_BASE_URL=<ferriskey-api-url>
FERRISKEY_ADMIN_USERNAME=<admin-user>
FERRISKEY_ADMIN_PASSWORD=<admin-password>
FERRISKEY_ALON_PASSWORD=<primary-test-password>
FERRISKEY_ALICE_PASSWORD=<secondary-test-password>
BIWORK_FERRISKEY_BASE_URL=<ferriskey-api-url>
BIWORK_FERRISKEY_USERNAME=<primary-test-user>
BIWORK_FERRISKEY_PASSWORD=<primary-test-password>
BIWORK_FERRISKEY_ALICE_USERNAME=<secondary-test-user>
BIWORK_FERRISKEY_ALICE_PASSWORD=<secondary-test-password>
BIWORK_RUST_API_URL=<rust-backend-url>
BIWORK_LIVE_CDP_URL=<electron-cdp-url>
PLAYWRIGHT_CHROME_EXECUTABLE=<chrome-executable>
```

FerrisKey 变量必须指向 API/OIDC 服务，不得误用 Web UI 地址。

### Google Maps MCP 与 Anthropic Skill

```dotenv
BIWORK_REAL_STREAMABLE_MCP_URL=<google-maps-mcp-url>
BIBI_TEST_STREAMABLE_MCP_URL=<same-google-maps-mcp-url>
BIWORK_REAL_SKILL_URL=<anthropic-skill-tree-url>
BIWORK_GENERIC_SKILL_URL=<anthropic-raw-skill-md-url>
```

Google Maps MCP 的 transport 固定为 `streamable-http`。Skill 生命周期测试使用 Anthropic 官方 Skills 仓库，不使用第三方 fork 或临时镜像。

### OpenAI-compatible 测试模型

```dotenv
DEFAULT_MODEL=<database-model-profile-name>
COMPATIBLE_MODEL=<provider-model-name>
COMPATIBLE_API_KEY=<api-key>
COMPATIBLE_BASE_URL=<compatible-api-base-url>
```

`DEFAULT_MODEL` 是数据库中的模型配置名称，不是 provider 的模型 ID。

### 可选真实依赖测试

```dotenv
RUSTFS_ENDPOINT=<s3-endpoint>
RUSTFS_ACCESS_KEY=<access-key>
RUSTFS_SECRET_KEY=<secret-key>
RUSTFS_REGION=<region>
RUSTFS_FILES_BUCKET=<files-bucket>
RUSTFS_AUDIT_BUCKET=<audit-bucket>
BIBI_TEST_QDRANT_REST_URL=<qdrant-url>
BIBI_TEST_EMBEDDING_ENDPOINT=<embedding-url>
BIBI_TEST_LEGACY_MCP_SSE_URL=<legacy-sse-url>
```

RustFS、Qdrant、Embedding 和 legacy SSE 测试仅在对应服务可用时配置。完整执行流程见 `docs/local-testing-guide.md`。
