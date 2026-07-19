# Observability stack

The compose stack connects the backend Prometheus endpoint to Alertmanager and a
generic enterprise webhook. Secrets are mounted from files and are never placed
in command-line arguments or committed configuration.

Create three local secret files outside the repository:

```bash
printf '%s' "$APP_INTERNAL__SHARED_TOKEN" > /tmp/bibi-internal-token
printf '%s' 'https://alerts.example.com/alertmanager' > /tmp/bibi-alert-webhook-url
printf '%s' "$BIBI_GRAFANA_ADMIN_PASSWORD" > /tmp/bibi-grafana-admin-password
chmod 600 /tmp/bibi-internal-token /tmp/bibi-alert-webhook-url /tmp/bibi-grafana-admin-password
```

Start the stack from the repository root:

```bash
BIBI_INTERNAL_TOKEN_FILE=/tmp/bibi-internal-token \
BIBI_ALERT_WEBHOOK_URL_FILE=/tmp/bibi-alert-webhook-url \
BIBI_GRAFANA_ADMIN_PASSWORD_FILE=/tmp/bibi-grafana-admin-password \
docker compose -f ops/observability.compose.yml up -d
```

Prometheus is exposed on port 9090, Alertmanager on port 9093, and Grafana on
port 3000. Grafana provisions Prometheus and the **BiWork SLO Overview**
dashboard automatically; sign in with `admin` (or `BIBI_GRAFANA_ADMIN_USER`)
and the password from the mounted secret file. The webhook
must accept Alertmanager's standard JSON payload. Critical alerts notify
immediately and repeat every 15 minutes; resolved notifications are enabled.

The stack records three 24-hour SLIs: run success, dispatch within ten seconds,
and tool execution within thirty seconds. Each target is 99 percent and alerts
only after at least 20 observations, so an idle or newly installed environment
does not page on a zero denominator.

Validate SLI expressions, alert timing, and the low-sample guard:

```bash
docker run --rm --entrypoint /bin/promtool \
  -v "$PWD/ops/prometheus:/rules:ro" prom/prometheus:latest \
  test rules /rules/bibi-work-rules.test.yml
```

Validate the complete Alertmanager-to-webhook path locally:

```bash
bash ops/tests/verify-alertmanager-webhook.sh
```
