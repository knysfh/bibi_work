#!/usr/bin/env bash
set -Eeuo pipefail

# Bibi Work local service supervisor.
# Starts: Rust backend, Python agent API, Python agent worker, Electron desktop.

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

BACKEND_DIR="$PROJECT_ROOT/bibi_work_backend"
AGENT_DIR="$PROJECT_ROOT/bibi_work_agent"
FRONTEND_DIR="${BIBI_FRONTEND_DIR:-$PROJECT_ROOT/bibi_work_frontend}"
LOG_DIR="$PROJECT_ROOT/logs"
PID_DIR="$PROJECT_ROOT/run"

BACKEND_PID_FILE="$PID_DIR/backend.pid"
AGENT_API_PID_FILE="$PID_DIR/agent-api.pid"
AGENT_WORKER_PID_FILE="$PID_DIR/agent-worker.pid"
DESKTOP_PID_FILE="$PID_DIR/desktop.pid"

BACKEND_LOG="$LOG_DIR/backend.log"
AGENT_API_LOG="$LOG_DIR/agent-api.log"
AGENT_WORKER_LOG="$LOG_DIR/agent-worker.log"
DESKTOP_LOG="$LOG_DIR/desktop.log"

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

mkdir -p "$LOG_DIR" "$PID_DIR"

load_env_file() {
    local file="$1"
    if [ -f "$file" ]; then
        set -a
        # shellcheck disable=SC1090
        . "$file"
        set +a
    fi
}

load_env() {
    load_env_file "$PROJECT_ROOT/.env"
    load_env_file "$PROJECT_ROOT/.env.local"
    load_env_file "$BACKEND_DIR/.env"
    load_env_file "$BACKEND_DIR/.env.local"
    load_env_file "$AGENT_DIR/.env"
    load_env_file "$AGENT_DIR/.env.local"
    load_env_file "$FRONTEND_DIR/.env"
    load_env_file "$FRONTEND_DIR/.env.local"
}

init_env() {
    export UV_CACHE_DIR="${UV_CACHE_DIR:-/tmp/uv-cache}"
    export APP_ENVIRONMENT="${APP_ENVIRONMENT:-local}"
    export APP_APPLICATION__PORT="${APP_APPLICATION__PORT:-8361}"
    export BIBI_AGENT__PORT="${BIBI_AGENT__PORT:-8371}"
    local local_no_proxy="127.0.0.1,localhost,::1"
    export NO_PROXY="${NO_PROXY:+$NO_PROXY,}$local_no_proxy"
    export no_proxy="${no_proxy:+$no_proxy,}$local_no_proxy"

    # Local-only fallback. Override from .env.local or the shell for shared environments.
    export APP_INTERNAL__SHARED_TOKEN="${APP_INTERNAL__SHARED_TOKEN:-${BIBI_WORK_INTERNAL_TOKEN:-local-internal-token}}"
    export APP_AGENT_RUNTIME__SHARED_TOKEN="${APP_AGENT_RUNTIME__SHARED_TOKEN:-$APP_INTERNAL__SHARED_TOKEN}"
    export BIBI_AGENT__INTERNAL_TOKEN="${BIBI_AGENT__INTERNAL_TOKEN:-$APP_INTERNAL__SHARED_TOKEN}"

    export APP_AGENT_RUNTIME__BASE_URL="${APP_AGENT_RUNTIME__BASE_URL:-http://127.0.0.1:${BIBI_AGENT__PORT}}"
    export BIBI_AGENT__RUST_BASE_URL="${BIBI_AGENT__RUST_BASE_URL:-http://127.0.0.1:${APP_APPLICATION__PORT}}"
    export BIBI_AGENT__DATABASE_URL="${BIBI_AGENT__DATABASE_URL:-${DATABASE_URL:-postgresql://postgres:password@127.0.0.1:5433/bibi_work}}"
    export BIBI_AGENT__CELERY_BROKER_URL="${BIBI_AGENT__CELERY_BROKER_URL:-redis://127.0.0.1:6380/1}"
    export BIBI_AGENT__CELERY_RESULT_BACKEND="${BIBI_AGENT__CELERY_RESULT_BACKEND:-redis://127.0.0.1:6380/2}"

    export VITE_BIBI_WORK_API_BASE_URL="${VITE_BIBI_WORK_API_BASE_URL:-http://127.0.0.1:${APP_APPLICATION__PORT}/api/v1}"
    export VITE_FERRISKEY_CLIENT_ID="${VITE_FERRISKEY_CLIENT_ID:-bibi-work-desktop}"
    export VITE_BIBI_WORK_REDIRECT_URI="${VITE_BIBI_WORK_REDIRECT_URI:-bibi-work://auth/callback}"
    export BIWORK_BACKEND_MODE="${BIWORK_BACKEND_MODE:-desktop-gateway}"
    export BIWORK_ENTERPRISE_BACKEND_URL="${BIWORK_ENTERPRISE_BACKEND_URL:-http://127.0.0.1:${APP_APPLICATION__PORT}}"
    if [ "$(uname -s)" = "Linux" ]; then
        export BIWORK_DISABLE_HARDWARE_ACCELERATION="${BIWORK_DISABLE_HARDWARE_ACCELERATION:-1}"
    fi
}

check_pid() {
    local pid_file="$1"
    if [ ! -f "$pid_file" ]; then
        return 1
    fi

    local pid
    pid="$(cat "$pid_file" 2>/dev/null || true)"
    if [[ "$pid" =~ ^[0-9]+$ ]] && kill -0 "$pid" >/dev/null 2>&1; then
        return 0
    fi

    local pgid
    pgid="$(read_recorded_pgid "$pid_file")"
    if [[ "$pgid" =~ ^[0-9]+$ ]] && kill -0 -- "-$pgid" >/dev/null 2>&1; then
        return 0
    fi

    return 1
}

service_health() {
    local service="$1"
    case "$service" in
        backend)
            curl -fsS --max-time 1 \
                "http://127.0.0.1:${APP_APPLICATION__PORT}/api/route-ownership" \
                >/dev/null 2>&1
            ;;
        agent-api)
            curl -fsS --max-time 1 \
                "http://127.0.0.1:${BIBI_AGENT__PORT}/health" \
                >/dev/null 2>&1
            ;;
        agent-worker)
            local output
            output="$({
                cd "$PROJECT_ROOT"
                uv run --project "$AGENT_DIR" celery \
                    -A bibi_work_agent.workers.celery_app:celery_app \
                    inspect ping --timeout=1
            } 2>/dev/null || true)"
            [[ "$output" == *pong* ]]
            ;;
        desktop)
            curl -fsS --max-time 1 \
                "${BIWORK_LIVE_CDP_URL:-http://127.0.0.1:${BIWORK_CDP_PORT:-9230}}/json/version" \
                >/dev/null 2>&1
            ;;
        *)
            return 1
            ;;
    esac
}

read_recorded_pgid() {
    local pid_file="$1"
    local pgid_file="${pid_file}.pgid"
    local pgid

    if [ ! -f "$pgid_file" ]; then
        return 0
    fi

    pgid="$(cat "$pgid_file" 2>/dev/null || true)"
    if [[ "$pgid" =~ ^[0-9]+$ ]]; then
        echo "$pgid"
    fi
}

remove_pid_files() {
    local pid_file="$1"
    rm -f "$pid_file" "${pid_file}.pgid" "${pid_file}.tmp" "${pid_file}.pgid.tmp"
}

require_command() {
    local command_name="$1"
    if ! command -v "$command_name" >/dev/null 2>&1; then
        echo -e "${RED}Missing command: $command_name${NC}" >&2
        exit 1
    fi
}

append_unique_service() {
    local service="$1"
    local existing
    for existing in "${SELECTED_SERVICES[@]:-}"; do
        if [ "$existing" = "$service" ]; then
            return
        fi
    done
    SELECTED_SERVICES+=("$service")
}

select_services() {
    local mode="$1"
    shift || true
    SELECTED_SERVICES=()

    if [ "$#" -eq 0 ]; then
        if [ "$mode" = "stop" ]; then
            SELECTED_SERVICES=(desktop agent-worker agent-api backend)
        else
            SELECTED_SERVICES=(backend agent-api agent-worker desktop)
        fi
        return
    fi

    local arg
    for arg in "$@"; do
        case "$arg" in
            all)
                if [ "$mode" = "stop" ]; then
                    append_unique_service desktop
                    append_unique_service agent-worker
                    append_unique_service agent-api
                    append_unique_service backend
                else
                    append_unique_service backend
                    append_unique_service agent-api
                    append_unique_service agent-worker
                    append_unique_service desktop
                fi
                ;;
            backend)
                append_unique_service backend
                ;;
            agent)
                if [ "$mode" = "stop" ]; then
                    append_unique_service agent-worker
                    append_unique_service agent-api
                else
                    append_unique_service agent-api
                    append_unique_service agent-worker
                fi
                ;;
            agent-api|agent-worker|desktop)
                append_unique_service "$arg"
                ;;
            ui)
                append_unique_service desktop
                ;;
            *)
                echo -e "${RED}Unknown service: $arg${NC}" >&2
                usage
                exit 1
                ;;
        esac
    done
}

start_process() {
    local name="$1"
    local pid_file="$2"
    local log_file="$3"
    local workdir="$4"
    local service="$5"
    shift 5

    if check_pid "$pid_file"; then
        echo "$name already running (PID: $(cat "$pid_file"))"
        return
    fi

    if service_health "$service"; then
        echo "$name already running (health check passed; recorded PID is not visible)"
        return
    fi

    if [ -f "$pid_file" ]; then
        remove_pid_files "$pid_file"
    fi

    echo "Starting $name..."
    {
        echo
        echo "[$(date '+%Y-%m-%d %H:%M:%S')] starting $name"
        echo "workdir: $workdir"
        printf 'command:'
        printf ' %q' "$@"
        echo
    } >>"$log_file"

    remove_pid_files "$pid_file"
    (
        cd "$workdir"
        if command -v setsid >/dev/null 2>&1; then
            # Do not record the outer background job's $!: util-linux setsid may
            # fork when that job is already a process-group leader. The new
            # session leader records its own stable PID/PGID before exec.
            exec setsid --fork bash -c '
                pid_file="$1"
                shift
                printf "%s\n" "$$" >"${pid_file}.tmp"
                mv -f "${pid_file}.tmp" "$pid_file"
                printf "%s\n" "$$" >"${pid_file}.pgid.tmp"
                mv -f "${pid_file}.pgid.tmp" "${pid_file}.pgid"
                exec "$@"
            ' bash "$pid_file" "$@" </dev/null
        fi
        exec nohup bash -c '
            pid_file="$1"
            shift
            printf "%s\n" "$$" >"${pid_file}.tmp"
            mv -f "${pid_file}.tmp" "$pid_file"
            exec "$@"
        ' bash "$pid_file" "$@" </dev/null
    ) >>"$log_file" 2>&1 &

    local i
    for i in {1..50}; do
        if check_pid "$pid_file"; then
            break
        fi
        sleep 0.1
    done
    sleep 1

    if check_pid "$pid_file"; then
        echo -e "${GREEN}$name started (PID: $(cat "$pid_file"))${NC}"
    elif service_health "$service"; then
        echo -e "${GREEN}$name started (health check passed; PID namespace is not visible)${NC}"
    else
        echo -e "${RED}$name failed to start. See $log_file${NC}"
        remove_pid_files "$pid_file"
        return 1
    fi
}

wait_for_http_ready() {
    local name="$1"
    local url="$2"
    local timeout_seconds="${3:-90}"
    local i

    if ! command -v curl >/dev/null 2>&1; then
        echo -e "${RED}Missing command: curl${NC}" >&2
        return 1
    fi

    echo "Waiting for $name to become ready ($url)..."
    for ((i = 1; i <= timeout_seconds; i++)); do
        if curl -fsS --max-time 2 "$url" >/dev/null 2>&1; then
            echo -e "${GREEN}$name is ready${NC}"
            return 0
        fi
        sleep 1
    done

    echo -e "${RED}$name did not become ready within ${timeout_seconds}s${NC}" >&2
    return 1
}

BACKEND_READY_WAITED=0

ensure_backend_ready_once() {
    if [ "$BACKEND_READY_WAITED" -eq 1 ]; then
        return 0
    fi
    wait_for_http_ready \
        "Backend" \
        "http://127.0.0.1:${APP_APPLICATION__PORT}/api/route-ownership" \
        "${BIBI_SERVICE_START_TIMEOUT:-90}"
    BACKEND_READY_WAITED=1
}

collect_descendants() {
    local parent_pid="$1"
    local child_pid
    for child_pid in $(pgrep -P "$parent_pid" 2>/dev/null || true); do
        collect_descendants "$child_pid"
        echo "$child_pid"
    done
}

find_desktop_main_pid() {
    local root_pid="$1"
    local child_pid
    local executable

    for child_pid in $(collect_descendants "$root_pid" | sort -n || true); do
        executable="$(readlink -f "/proc/$child_pid/exe" 2>/dev/null || true)"
        if [ "${executable##*/}" = "electron" ]; then
            echo "$child_pid"
            return 0
        fi
    done
}

stop_process() {
    local name="$1"
    local pid_file="$2"
    local service="${3:-}"

    if [ ! -f "$pid_file" ]; then
        echo "$name stopped (no PID file)"
        return
    fi

    local pid
    local pgid
    pid="$(cat "$pid_file" 2>/dev/null || true)"
    pgid="$(read_recorded_pgid "$pid_file")"

    local pid_alive=0
    local group_alive=0
    if [[ "$pid" =~ ^[0-9]+$ ]] && kill -0 "$pid" >/dev/null 2>&1; then
        pid_alive=1
    fi
    if [[ "$pgid" =~ ^[0-9]+$ ]] && kill -0 -- "-$pgid" >/dev/null 2>&1; then
        group_alive=1
    fi

    if [ "$pid_alive" -eq 0 ] && [ "$group_alive" -eq 0 ]; then
        if [ -n "$service" ] && service_health "$service"; then
            echo -e "${RED}$name is running, but PID/PGID $pid/$pgid is not signalable from this namespace.${NC}" >&2
            echo -e "${RED}Refusing to remove its PID files or pretend it stopped.${NC}" >&2
            return 1
        fi
        echo "$name process is gone; removing stale PID file"
        remove_pid_files "$pid_file"
        return
    fi

    local descendants=""
    if [ "$pid_alive" -eq 1 ]; then
        descendants="$(collect_descendants "$pid" | sort -u || true)"
    fi

    local graceful_pid=""
    if [ "$service" = "desktop" ] && [ "$pid_alive" -eq 1 ]; then
        graceful_pid="$(find_desktop_main_pid "$pid")"
    fi

    if [ -n "$graceful_pid" ]; then
        echo "Stopping $name gracefully (Electron PID: $graceful_pid, PGID: $pgid)..."
        kill -TERM "$graceful_pid" >/dev/null 2>&1 || true
    elif [ "$group_alive" -eq 1 ]; then
        echo "Stopping $name (PID: $pid, PGID: $pgid)..."
        kill -TERM -- "-$pgid" >/dev/null 2>&1 || true
    elif [ "$pid_alive" -eq 1 ]; then
        echo "Stopping $name (PID: $pid)..."
        kill -TERM "$pid" $descendants >/dev/null 2>&1 || true
    fi

    local i
    for i in {1..10}; do
        pid_alive=0
        group_alive=0
        if [[ "$pid" =~ ^[0-9]+$ ]] && kill -0 "$pid" >/dev/null 2>&1; then
            pid_alive=1
        fi
        if [[ "$pgid" =~ ^[0-9]+$ ]] && kill -0 -- "-$pgid" >/dev/null 2>&1; then
            group_alive=1
        fi
        if [ "$pid_alive" -eq 0 ] && [ "$group_alive" -eq 0 ]; then
            break
        fi
        sleep 1
    done

    if [ "$pid_alive" -eq 1 ] || [ "$group_alive" -eq 1 ]; then
        echo -e "${YELLOW}$name did not stop cleanly; sending SIGKILL${NC}"
        if [ "$group_alive" -eq 1 ]; then
            kill -KILL -- "-$pgid" >/dev/null 2>&1 || true
        fi
        if [ "$pid_alive" -eq 1 ]; then
            kill -KILL "$pid" $descendants >/dev/null 2>&1 || true
        fi
    fi

    remove_pid_files "$pid_file"
    echo -e "${GREEN}$name stopped${NC}"
}

service_label() {
    case "$1" in
        backend) echo "Backend" ;;
        agent-api) echo "Agent API" ;;
        agent-worker) echo "Agent Worker" ;;
        desktop) echo "Desktop" ;;
    esac
}

pid_file_for() {
    case "$1" in
        backend) echo "$BACKEND_PID_FILE" ;;
        agent-api) echo "$AGENT_API_PID_FILE" ;;
        agent-worker) echo "$AGENT_WORKER_PID_FILE" ;;
        desktop) echo "$DESKTOP_PID_FILE" ;;
    esac
}

log_file_for() {
    case "$1" in
        backend) echo "$BACKEND_LOG" ;;
        agent-api) echo "$AGENT_API_LOG" ;;
        agent-worker) echo "$AGENT_WORKER_LOG" ;;
        desktop) echo "$DESKTOP_LOG" ;;
    esac
}

start_one() {
    case "$1" in
        backend)
            require_command cargo
            start_process "Backend" "$BACKEND_PID_FILE" "$BACKEND_LOG" "$BACKEND_DIR" backend cargo run
            ;;
        agent-api)
            require_command uv
            start_process "Agent API" "$AGENT_API_PID_FILE" "$AGENT_API_LOG" "$PROJECT_ROOT" agent-api \
                uv run --project "$AGENT_DIR" uvicorn bibi_work_agent.main:app \
                --host 0.0.0.0 --port "$BIBI_AGENT__PORT"
            ;;
        agent-worker)
            require_command uv
            start_process "Agent Worker" "$AGENT_WORKER_PID_FILE" "$AGENT_WORKER_LOG" "$PROJECT_ROOT" agent-worker \
                uv run --project "$AGENT_DIR" celery \
                -A bibi_work_agent.workers.celery_app:celery_app worker \
                --loglevel="${BIBI_AGENT__CELERY_LOGLEVEL:-info}" \
                --concurrency="${BIBI_AGENT__WORKER_CONCURRENCY:-1}" \
                -n "bibi-work-agent@%h"
            ;;
        desktop)
            require_command bun
            case "${BIWORK_DESKTOP_MODE:-dev}" in
                dev)
                    start_process "Desktop" "$DESKTOP_PID_FILE" "$DESKTOP_LOG" "$FRONTEND_DIR" desktop bun start
                    ;;
                preview)
                    start_process "Desktop Preview" "$DESKTOP_PID_FILE" "$DESKTOP_LOG" "$FRONTEND_DIR" desktop \
                        bun run start:preview
                    ;;
                *)
                    echo -e "${RED}Unsupported BIWORK_DESKTOP_MODE: ${BIWORK_DESKTOP_MODE}${NC}" >&2
                    return 1
                    ;;
            esac
            ;;
    esac
}

stop_one() {
    stop_process "$(service_label "$1")" "$(pid_file_for "$1")" "$1"
}

status_one() {
    local service="$1"
    local label
    local pid_file
    label="$(service_label "$service")"
    pid_file="$(pid_file_for "$service")"

    if check_pid "$pid_file"; then
        printf "%-14s %bRUNNING%b  PID=%s\n" "$label:" "$GREEN" "$NC" "$(cat "$pid_file")"
    elif service_health "$service"; then
        printf "%-14s %bRUNNING%b  PID=%s (healthy; PID namespace unavailable)\n" \
            "$label:" "$GREEN" "$NC" "$(cat "$pid_file" 2>/dev/null || echo unknown)"
    else
        printf "%-14s %bSTOPPED%b\n" "$label:" "$RED" "$NC"
    fi
}

warn_config() {
    if [ "$APP_INTERNAL__SHARED_TOKEN" != "$APP_AGENT_RUNTIME__SHARED_TOKEN" ]; then
        echo -e "${YELLOW}Warning: APP_INTERNAL__SHARED_TOKEN and APP_AGENT_RUNTIME__SHARED_TOKEN differ.${NC}"
    fi

    if [ "$APP_INTERNAL__SHARED_TOKEN" != "$BIBI_AGENT__INTERNAL_TOKEN" ]; then
        echo -e "${YELLOW}Warning: APP_INTERNAL__SHARED_TOKEN and BIBI_AGENT__INTERNAL_TOKEN differ.${NC}"
    fi

    if [ "$APP_INTERNAL__SHARED_TOKEN" = "local-internal-token" ]; then
        echo -e "${YELLOW}Warning: using local default internal token. Override it in .env.local for shared machines.${NC}"
    fi

    if [ -z "${OPENAI_API_KEY:-}" ] &&
        [ -z "${ANTHROPIC_API_KEY:-}" ] &&
        [ -z "${GOOGLE_API_KEY:-}" ] &&
        [ -z "${COMPATIBLE_API_KEY:-}" ]; then
        echo -e "${YELLOW}Warning: no common LLM API key env detected. Services can start, but real model runs may fail.${NC}"
    fi
}

show_config() {
    cat <<EOF
Project root:                 $PROJECT_ROOT
Backend API:                  http://127.0.0.1:${APP_APPLICATION__PORT}/api/v1
Agent API:                    http://127.0.0.1:${BIBI_AGENT__PORT}
Backend -> agent runtime:      $APP_AGENT_RUNTIME__BASE_URL
Agent -> backend:              $BIBI_AGENT__RUST_BASE_URL
Frontend API base URL:         $VITE_BIBI_WORK_API_BASE_URL
FerrisKey desktop client ID:   $VITE_FERRISKEY_CLIENT_ID
FerrisKey redirect URI:        $VITE_BIBI_WORK_REDIRECT_URI
Internal token:                set
Agent DB URL:                  ${BIBI_AGENT__DATABASE_URL}
Agent Celery broker:           ${BIBI_AGENT__CELERY_BROKER_URL}
Logs:                          $LOG_DIR
PIDs:                          $PID_DIR
EOF
    warn_config
}

check_commands() {
    require_command cargo
    require_command uv
    require_command bun
    require_command pgrep
    echo -e "${GREEN}Required commands are available.${NC}"
    warn_config
}

start_app() {
    select_services start "$@"
    warn_config
    BACKEND_READY_WAITED=0
    local service
    for service in "${SELECTED_SERVICES[@]}"; do
        if [ "$service" = "desktop" ]; then
            ensure_backend_ready_once
        fi
        start_one "$service"
        if [ "$service" = "backend" ]; then
            ensure_backend_ready_once
        fi
    done
}

stop_app() {
    select_services stop "$@"
    local service
    for service in "${SELECTED_SERVICES[@]}"; do
        stop_one "$service"
    done
}

status_app() {
    select_services status "$@"
    local service
    for service in "${SELECTED_SERVICES[@]}"; do
        status_one "$service"
    done
    echo "Logs: $LOG_DIR"
}

tail_logs() {
    select_services logs "$@"
    local args=()
    local service
    for service in "${SELECTED_SERVICES[@]}"; do
        args+=("$(log_file_for "$service")")
    done
    tail -n "${LOG_LINES:-120}" -f "${args[@]}"
}

usage() {
    cat <<EOF
Usage: $0 {start|stop|restart|status|config|check|logs} [all|backend|agent|agent-api|agent-worker|desktop|ui]

Default service set:
  backend agent-api agent-worker desktop

Examples:
  $0 start
  $0 restart backend agent
  $0 start ui
  $0 logs agent
EOF
}

load_env
init_env

case "${1:-}" in
    start)
        shift
        start_app "$@"
        ;;
    stop)
        shift
        stop_app "$@"
        ;;
    restart)
        shift
        stop_app "$@"
        sleep 2
        start_app "$@"
        ;;
    status)
        shift
        status_app "$@"
        ;;
    config)
        show_config
        ;;
    check)
        check_commands
        ;;
    logs)
        shift
        tail_logs "$@"
        ;;
    *)
        usage
        exit 1
        ;;
esac
