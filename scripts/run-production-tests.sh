#!/usr/bin/env bash
set -Eeuo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-e2e}"

load_env_file() {
    local file="$1"
    if [ -f "$file" ]; then
        set -a
        # shellcheck disable=SC1090
        . "$file"
        set +a
    fi
}

load_test_env() {
    load_env_file "$PROJECT_ROOT/bibi_work_backend/.env"
    load_env_file "$PROJECT_ROOT/bibi_work_backend/.env.local"
}

require_test_env() {
    local required=(
        DATABASE_URL
        APP_INTERNAL__SHARED_TOKEN
        BIBI_AGENT__INTERNAL_TOKEN
        BIWORK_FERRISKEY_PASSWORD
        BIWORK_LIVE_CDP_URL
        COMPATIBLE_API_KEY
        COMPATIBLE_BASE_URL
        COMPATIBLE_MODEL
        DEFAULT_MODEL
        BIWORK_REAL_STREAMABLE_MCP_URL
        BIWORK_REAL_SKILL_URL
    )
    local missing=()
    local name
    for name in "${required[@]}"; do
        if [ -z "${!name:-}" ]; then
            missing+=("$name")
        fi
    done
    if [ "${#missing[@]}" -gt 0 ]; then
        printf 'Missing required test variable: %s\n' "${missing[@]}" >&2
        exit 1
    fi
}

ensure_live_services() {
    "$PROJECT_ROOT/services.sh" check

    local rust_api="${BIWORK_RUST_API_URL:-http://127.0.0.1:8361}"
    local agent_api="${APP_AGENT_RUNTIME__BASE_URL:-http://127.0.0.1:8371}"
    if ! curl -fsS "$rust_api/api/route-ownership" >/dev/null; then
        "$PROJECT_ROOT/services.sh" start backend
    fi
    ensure_production_desktop
    if ! curl -fsS "$agent_api/health" >/dev/null; then
        "$PROJECT_ROOT/services.sh" start agent-api
    fi
    if ! uv run --project "$PROJECT_ROOT/bibi_work_agent" celery \
        -A bibi_work_agent.workers.celery_app:celery_app inspect ping --timeout=5 >/dev/null 2>&1; then
        "$PROJECT_ROOT/services.sh" start agent-worker
    fi
    "$PROJECT_ROOT/services.sh" status
    curl -fsS "$rust_api/api/route-ownership" >/dev/null
    curl -fsS "$agent_api/health" >/dev/null
    curl -fsS "${BIWORK_LIVE_CDP_URL%/}/json/version" >/dev/null
}

ensure_production_desktop() {
    "$PROJECT_ROOT/services.sh" stop desktop
    (
        cd "$PROJECT_ROOT/bibi_work_frontend"
        bun run package
    )
    BIWORK_E2E_TEST=1 BIWORK_DESKTOP_MODE=preview "$PROJECT_ROOT/services.sh" start desktop

    local cdp_url="${BIWORK_LIVE_CDP_URL%/}/json/version"
    local attempt
    for attempt in $(seq 1 60); do
        if curl -fsS --max-time 2 "$cdp_url" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    echo "Desktop preview CDP did not become ready" >&2
    return 1
}

run_live_acceptance() {
    (
        cd "$PROJECT_ROOT/bibi_work_frontend"
        bun run test:e2e:production
    )
}

run_deterministic_regression() {
    (
        cd "$PROJECT_ROOT/bibi_work_backend"
        cargo fmt --check
        cargo test
        cargo clippy --all-targets -- -D warnings
    )
    (
        cd "$PROJECT_ROOT/bibi_work_agent"
        uv run pytest
    )
    (
        cd "$PROJECT_ROOT/bibi_work_frontend"
        bun run lint
        bun run test
    )
}

case "$MODE" in
    smoke|e2e)
        load_test_env
        require_test_env
        ensure_live_services
        run_live_acceptance
        ;;
    regression)
        load_test_env
        require_test_env
        run_deterministic_regression
        ensure_live_services
        run_live_acceptance
        ;;
    *)
        echo "Usage: $0 [smoke|e2e|regression]" >&2
        exit 2
        ;;
esac
