# 本地 E2E、冒烟与全量回归测试

本文档是项目本地测试的标准执行流程。真实值只存在于 `docs/local-service-config.local.md` 和 `bibi_work_backend/.env.local`，不要把它们复制到命令输出、日志、快照或回复。

## 1. 准备并导入环境

在仓库根目录执行：

```bash
test -f docs/local-service-config.local.md
test -f bibi_work_backend/.env.local

set -a
. bibi_work_backend/.env
. bibi_work_backend/.env.local
set +a
```

`services.sh` 会自动加载相同文件；显式导入用于随后直接运行 `cargo`、`uv`、`bun`、Playwright 和 bootstrap 脚本。

仅检查变量是否存在，不输出值：

```bash
required=(
  DATABASE_URL
  APP_INTERNAL__SHARED_TOKEN
  BIBI_AGENT__INTERNAL_TOKEN
  BIWORK_FERRISKEY_PASSWORD
  COMPATIBLE_API_KEY
  COMPATIBLE_BASE_URL
  COMPATIBLE_MODEL
)
for name in "${required[@]}"; do
  test -n "${!name:-}" || { echo "missing: ${name}" >&2; exit 1; }
done
```

## 2. 依赖和数据库

基础依赖包括 PostgreSQL、Redis、FerrisKey；真实文件集成测试还需要 RustFS；MCP/Skill E2E 还需要 Google Maps MCP、网络访问和 Chrome。

```bash
./services.sh check
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -c 'select 1'
redis-cli -u "$BIBI_AGENT__CELERY_BROKER_URL" ping
```

只有在任务明确要求重建数据库时才删除数据库。新库初始化流程是：创建空库、执行当前 migrations、再同步 FerrisKey 权限数据。无需也不得手工运行历史 SQL 文件。

```bash
cd bibi_work_backend
sqlx database create
sqlx migrate run
python3 scripts/bootstrap_ferriskey.py
cd ..
```

`bootstrap_ferriskey.py` 是幂等步骤；数据库已存在时可以直接再次执行。删除数据库是破坏性操作，必须由当前任务明确授权后再做。

## 3. 启动与基础冒烟

```bash
./services.sh start
./services.sh status

curl -fsS http://127.0.0.1:8361/api/route-ownership >/dev/null
curl -fsS http://127.0.0.1:8371/health >/dev/null
curl -fsS "${BIWORK_LIVE_CDP_URL}/json/version" >/dev/null
```

若改过运行配置，使用 `./services.sh restart`，不要只重启其中一个仍依赖旧环境的进程。排错时读取 `logs/`，但不得回显请求凭证或 `.env.local`。

## 4. 生产级 Live 冒烟与 E2E

统一入口位于仓库根目录。`smoke` 与 `e2e` 使用同一套真实链路验收，避免维护两套行为不同、结论不可比的脚本：

```bash
./scripts/run-production-tests.sh smoke
./scripts/run-production-tests.sh e2e
```

统一入口会先停止开发态 Electron，执行 `bun run package`，再以 `electron-vite preview` 启动已构建桌面端。这样验收的是生产构建产物，同时避免 Vite 开发服务器与 Electron renderer 长时间叠加占用内存。直接使用 `./services.sh start` 时仍保持开发模式；如需手工启动已有构建产物，可执行：

```bash
BIWORK_DESKTOP_MODE=preview ./services.sh start desktop
```

这套验收固定使用真实 OpenAI-compatible 模型，并覆盖：

1. 同一会话连续完成 5 轮对话，逐轮校验上下文 token 和真实流式渲染。
2. 切换到另一会话，让模型生成约 200 字的中文 Python 学习计划，把模型生成内容保存为 Markdown 后执行 UI 预览并截图。
3. 再切回原会话，reload 后逐项确认 5 轮历史仍可见并截图。
4. 注册并发现真实 Google Maps `streamable-http` MCP，让模型通过会话查询“北京站的经纬度”，校验 `maps_geocode` 工具调用、经纬度结果和截图。
5. 禁用同一 MCP 后再次查询，校验模型得到 `MCP_DISABLED_NO_TOOL`，且 conversation run events 中没有新增 MCP tool call，并截图。
6. 使用本地 HTTP fixture 启动隔离的持久化浏览器 Profile，校验 headed 执行协议、snapshot、密码输入拦截和 session 关闭。

前端单独执行同一套生产用例时使用：

```bash
cd bibi_work_frontend
bun run test:e2e:production
```

只验证浏览器执行器时使用：

```bash
cd bibi_work_frontend
BIWORK_BROWSER_HEADLESS=1 bun run test:e2e:browser
```

该入口默认串行执行聚焦的 `production-conversation-journey.e2e.ts` 和真实 MCP 生命周期用例。完整企业导航、团队、定时任务和治理长流程单独执行：

```bash
bun run test:e2e:enterprise:production
```

只调试 Google Maps MCP 与 Anthropic Skill 生命周期时：

```bash
bunx playwright test \
  --config playwright.config.ts \
  tests/e2e/specs/enterprise-mcp-skill-lifecycle.e2e.ts \
  --reporter=line
```

远程 Anthropic `SKILL.md` 与 ZIP 导入：

```bash
bunx playwright test \
  --config playwright.config.ts \
  tests/e2e/specs/skill-import-sources-live.e2e.ts \
  --reporter=line
```

上述 MCP 测试要求：

- `BIWORK_REAL_STREAMABLE_MCP_URL` 指向本机 Google Maps MCP 的 `/mcp` 端点。
- 注册 transport 使用 canonical 值 `streamable-http`；`streamable_http` 只允许作为兼容输入，不应保存为数据库事实。
- `BIWORK_REAL_SKILL_URL` 指向 [Anthropic 官方 `skills/mcp-builder` 目录](https://github.com/anthropics/skills/tree/main/skills/mcp-builder)。
- `BIWORK_GENERIC_SKILL_URL` 指向同一 Skill 的官方原始 `SKILL.md`。

需要运行全部确定性 Playwright case 时：

```bash
bun run test:e2e
```

完整 E2E 共用单个 Electron 实例，Playwright 配置固定 `workers: 1`，不要为了提速并行运行这些 case。生产验收也必须串行，因为第二个用例会复用同一个 Electron 登录态并验证 MCP 启停后的权威状态。

## 5. 标准全量回归

标准全量回归可由一个命令完成；它先运行确定性测试，再启动/确认完整服务并执行第 4 节的真实模型生产验收：

```bash
./scripts/run-production-tests.sh regression
```

脚本内部依次执行以下确定性测试：

```bash
cd bibi_work_backend
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings

cd ../bibi_work_agent
uv run pytest

cd ../bibi_work_frontend
bun run lint
bun run test
```

随后脚本会确认完整服务正在运行，并执行第 4 节的五轮对话、历史恢复、模型内容预览和 MCP 启停 live E2E。不要用 `cargo test -- --ignored` 一次性运行全部 ignored 测试，因为它们要求不同的外部服务，部分还会修改共享测试数据。

真实 streamable MCP 与 Anthropic Skill 的 Rust 定向测试为：

```bash
cd bibi_work_backend
cargo test real_streamable_http_session_lists_and_calls_tool -- --ignored --nocapture
cargo test discovers_tools_from_real_streamable_http_server -- --ignored --nocapture
cargo test downloads_a_pinned_example_skill_document -- --ignored --nocapture
```

RustFS、Qdrant、Embedding、legacy SSE 等 ignored 测试只在对应变量和服务齐全时定向运行，要求见 `docs/local-service-config.md`。

## 6. 结果判定与收尾

通过标准：命令退出码为 0，服务无 panic/traceback，Playwright 无失败；真实模型完成 5 轮上下文对话；切换会话并 reload 后历史完整；模型生成的约 200 字 Python 学习计划能在预览面板正确显示；启用 MCP 时模型调用 Google Maps `maps_geocode` 并返回北京站经纬度；禁用后没有新增 MCP tool call；Skill 来源解析为 Anthropic 官方仓库；模型调用不出现认证、模型不存在或 base URL 错误。

完成后按需停止服务：

```bash
"$(git rev-parse --show-toplevel)/services.sh" stop
```

测试报告只记录命令、通过数、失败测试名和脱敏错误。禁止记录密码、API Key、token、Authorization header 或本机配置文件内容。
