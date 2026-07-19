#!/usr/bin/env bash
set -Eeuo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TEMP_DIR="$(mktemp -d)"
CONTAINER_NAME="bibi-alertmanager-contract-$$"
WEBHOOK_PORT="19099"
ALERTMANAGER_PORT="19093"
PAYLOAD_FILE="$TEMP_DIR/webhook-payload.json"

cleanup() {
    docker stop "$CONTAINER_NAME" >/dev/null 2>&1 || true
    if [ -n "${WEBHOOK_PID:-}" ]; then
        kill "$WEBHOOK_PID" >/dev/null 2>&1 || true
    fi
    rm -rf "$TEMP_DIR"
}
trap cleanup EXIT

printf '%s' "http://host.docker.internal:${WEBHOOK_PORT}/alerts" >"$TEMP_DIR/webhook-url"
node "$PROJECT_ROOT/ops/tests/alertmanager-webhook-fixture.mjs" \
    "$WEBHOOK_PORT" "$PAYLOAD_FILE" &
WEBHOOK_PID="$!"

docker run --rm -d \
    --name "$CONTAINER_NAME" \
    --add-host host.docker.internal:host-gateway \
    -p "${ALERTMANAGER_PORT}:9093" \
    -v "$PROJECT_ROOT/ops/alertmanager/alertmanager.yml:/etc/alertmanager/alertmanager.yml:ro" \
    -v "$TEMP_DIR/webhook-url:/run/secrets/bibi_alert_webhook_url:ro" \
    prom/alertmanager:v0.28.1 \
    --config.file=/etc/alertmanager/alertmanager.yml >/dev/null

for _ in $(seq 1 50); do
    if curl -fsS "http://127.0.0.1:${ALERTMANAGER_PORT}/-/ready" >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done
curl -fsS "http://127.0.0.1:${ALERTMANAGER_PORT}/-/ready" >/dev/null

curl -fsS \
    -H 'content-type: application/json' \
    -d '[{"labels":{"alertname":"BibiWebhookContract","severity":"critical"},"annotations":{"summary":"contract delivery"}}]' \
    "http://127.0.0.1:${ALERTMANAGER_PORT}/api/v2/alerts" >/dev/null

for _ in $(seq 1 100); do
    if [ -s "$PAYLOAD_FILE" ]; then
        break
    fi
    sleep 0.1
done

node -e '
const payload = JSON.parse(require("node:fs").readFileSync(process.argv[1], "utf8"));
if (payload.receiver !== "enterprise-webhook") throw new Error("unexpected receiver");
const alert = payload.alerts?.find((item) => item.labels?.alertname === "BibiWebhookContract");
if (!alert || alert.labels.severity !== "critical" || alert.status !== "firing") {
  throw new Error("critical firing alert was not delivered");
}
' "$PAYLOAD_FILE"

echo "Alertmanager webhook contract passed"
