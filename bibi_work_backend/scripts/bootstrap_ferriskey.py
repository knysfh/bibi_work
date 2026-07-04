#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from typing import Any


DEFAULT_ADMIN_ROLES = [
    "platform_admin",
    "tenant_admin",
    "security_admin",
    "audit_admin",
    "agent_admin",
    "skill_admin",
    "mcp_admin",
    "tool_admin",
    "workflow_admin",
    "memory_admin",
    "project_admin",
    "local_exec_admin",
]

DEFAULT_ALICE_ROLES = [
    "tenant_member",
    "agent_runner",
    "workflow_operator",
    "skill_user",
    "mcp_user",
    "tool_user_low",
    "personal_space_owner",
]

ROLE_NAMES = sorted(
    set(
        DEFAULT_ADMIN_ROLES
        + DEFAULT_ALICE_ROLES
        + [
            "dept_academic_affairs_member",
            "dept_academic_affairs_approver",
            "dept_library_member",
            "dept_library_approver",
            "class_advisor",
            "class_advisor_approver",
            "tool_user_medium",
        ]
    )
)


@dataclass(frozen=True)
class Settings:
    ferriskey_base_url: str
    admin_username: str
    admin_password: str
    admin_client_id: str
    master_realm: str
    realm: str
    tenant_slug: str
    database_url: str


def main() -> None:
    settings = Settings(
        ferriskey_base_url=os.getenv("FERRISKEY_BASE_URL", "http://localhost:3333").rstrip("/"),
        admin_username=os.getenv("FERRISKEY_ADMIN_USERNAME", "admin"),
        admin_password=os.getenv("FERRISKEY_ADMIN_PASSWORD", "admin"),
        admin_client_id=os.getenv("FERRISKEY_ADMIN_CLIENT_ID", "admin-cli"),
        master_realm=os.getenv("FERRISKEY_MASTER_REALM", "master"),
        realm=os.getenv("FERRISKEY_REALM", "bibi-work"),
        tenant_slug=os.getenv("BIBI_WORK_TENANT_SLUG", "bibi-work"),
        database_url=os.getenv(
            "DATABASE_URL", "postgresql://postgres:password@127.0.0.1:5433/bibi_work"
        ),
    )

    token = admin_token(settings)
    ensure_realm(settings, token)
    for client_id in client_ids():
        ensure_client(settings, token, client_id)
    role_reps = {role: ensure_role(settings, token, role) for role in ROLE_NAMES}

    alon = ensure_user(
        settings,
        token,
        username="alon",
        email=os.getenv("FERRISKEY_ALON_EMAIL", "alon@example.local"),
        display_name=os.getenv("FERRISKEY_ALON_DISPLAY_NAME", "Alon"),
        password=os.getenv("FERRISKEY_ALON_PASSWORD"),
    )
    alice = ensure_user(
        settings,
        token,
        username="alice",
        email=os.getenv("FERRISKEY_ALICE_EMAIL", "alice@example.local"),
        display_name=os.getenv("FERRISKEY_ALICE_DISPLAY_NAME", "Alice"),
        password=os.getenv("FERRISKEY_ALICE_PASSWORD"),
    )

    assign_realm_roles(settings, token, alon["id"], [role_reps[name] for name in DEFAULT_ADMIN_ROLES])
    assign_realm_roles(settings, token, alice["id"], [role_reps[name] for name in DEFAULT_ALICE_ROLES])
    sync_postgres(settings, alon, alice)
    print("FerrisKey and Postgres bootstrap completed.")


def client_ids() -> list[str]:
    raw = os.getenv(
        "FERRISKEY_CLIENT_IDS",
        "bibi-work-web,bibi-work-desktop,bibi-work-backend,bibi-work-runtime",
    )
    return [item.strip() for item in raw.split(",") if item.strip()]


def client_redirect_uris(client_id: str) -> list[str]:
    if client_id == "bibi-work-desktop":
        return [
            os.getenv(
                "FERRISKEY_DESKTOP_REDIRECT_URI",
                "bibi-work://auth/callback",
            )
        ]
    return []


def admin_token(settings: Settings) -> str:
    payload = urllib.parse.urlencode(
        {
            "grant_type": "password",
            "client_id": settings.admin_client_id,
            "username": settings.admin_username,
            "password": settings.admin_password,
        }
    ).encode()
    data = http_json(
        "POST",
        f"{settings.ferriskey_base_url}/realms/{settings.master_realm}/protocol/openid-connect/token",
        data=payload,
        headers={"content-type": "application/x-www-form-urlencoded"},
    )
    token = data.get("access_token")
    if not token:
        raise RuntimeError("FerrisKey admin token response did not include access_token")
    return token


def ensure_realm(settings: Settings, token: str) -> None:
    path = f"/realms/{settings.realm}"
    if http_maybe_json(settings, token, "GET", path) is not None:
        return
    http_json(
        "POST",
        f"{settings.ferriskey_base_url}/realms",
        token=token,
        json_body={"name": settings.realm},
        expected=(201, 200),
    )


def ensure_client(settings: Settings, token: str, client_id: str) -> None:
    clients = response_data(
        http_json(
            "GET",
            f"{settings.ferriskey_base_url}/realms/{settings.realm}/clients",
            token=token,
        )
    )
    existing = next(
        (client for client in clients if client.get("client_id") == client_id),
        None,
    )
    public_client = client_id.endswith("-web") or client_id.endswith("-desktop")
    # Password grant is enabled for backend only to support local CLI smoke tests.
    direct_access = client_id in {"bibi-work-backend", "admin-cli"}
    client_body = {
        "enabled": True,
        "direct_access_grants_enabled": direct_access,
        "name": client_id,
    }
    if existing is not None:
        http_json(
            "PATCH",
            f"{settings.ferriskey_base_url}/realms/{settings.realm}/clients/{existing['id']}",
            token=token,
            json_body=client_body,
            expected=(200,),
        )
        ensure_client_redirect_uris(settings, token, existing["id"], client_id)
        return

    body = {
        "client_id": client_id,
        "name": client_id,
        "enabled": True,
        "protocol": "openid-connect",
        "public_client": public_client,
        "client_type": "public" if public_client else "confidential",
        "direct_access_grants_enabled": direct_access,
        "service_account_enabled": not public_client,
    }
    http_json(
        "POST",
        f"{settings.ferriskey_base_url}/realms/{settings.realm}/clients",
        token=token,
        json_body=body,
        expected=(201,),
    )
    clients = response_data(
        http_json(
            "GET",
            f"{settings.ferriskey_base_url}/realms/{settings.realm}/clients",
            token=token,
        )
    )
    created = next(
        (client for client in clients if client.get("client_id") == client_id),
        None,
    )
    if created is None:
        raise RuntimeError(f"client was not readable after create: {client_id}")
    ensure_client_redirect_uris(settings, token, created["id"], client_id)


def ensure_client_redirect_uris(
    settings: Settings, token: str, ferriskey_client_id: str, client_id: str
) -> None:
    redirect_uris = client_redirect_uris(client_id)
    if not redirect_uris:
        return
    existing = response_data(
        http_json(
            "GET",
            f"{settings.ferriskey_base_url}/realms/{settings.realm}/clients/{ferriskey_client_id}/redirects",
            token=token,
        )
    )
    existing_values = {item.get("value") for item in existing}
    for redirect_uri in redirect_uris:
        if redirect_uri in existing_values:
            continue
        http_json(
            "POST",
            f"{settings.ferriskey_base_url}/realms/{settings.realm}/clients/{ferriskey_client_id}/redirects",
            token=token,
            json_body={"value": redirect_uri, "enabled": True},
            expected=(201, 200),
        )


def ensure_role(settings: Settings, token: str, role_name: str) -> dict[str, Any]:
    roles = response_data(
        http_json(
            "GET",
            f"{settings.ferriskey_base_url}/realms/{settings.realm}/roles",
            token=token,
        )
    )
    existing = next((role for role in roles if role.get("name") == role_name), None)
    if existing is not None:
        return existing
    http_json(
        "POST",
        f"{settings.ferriskey_base_url}/realms/{settings.realm}/roles",
        token=token,
        json_body={"name": role_name, "permissions": []},
        expected=(201,),
    )
    roles = response_data(
        http_json(
            "GET",
            f"{settings.ferriskey_base_url}/realms/{settings.realm}/roles",
            token=token,
        )
    )
    created = next((role for role in roles if role.get("name") == role_name), None)
    if created is None:
        raise RuntimeError(f"role was not readable after create: {role_name}")
    return created


def ensure_user(
    settings: Settings,
    token: str,
    *,
    username: str,
    email: str,
    display_name: str,
    password: str | None,
) -> dict[str, Any]:
    existing = find_user(settings, token, username)
    if existing is not None:
        if password:
            reset_user_password(settings, token, existing["id"], password)
        return existing
    if not password:
        raise RuntimeError(
            f"FERRISKEY_{username.upper()}_PASSWORD is required to create user {username}"
        )
    body = {
        "username": username,
        "email": email,
        "firstname": display_name,
        "email_verified": True,
    }
    http_json(
        "POST",
        f"{settings.ferriskey_base_url}/realms/{settings.realm}/users",
        token=token,
        json_body=body,
        expected=(201,),
    )
    created = find_user(settings, token, username)
    if created is None:
        raise RuntimeError(f"user was not readable after create: {username}")
    reset_user_password(settings, token, created["id"], password)
    return created


def find_user(settings: Settings, token: str, username: str) -> dict[str, Any] | None:
    users = response_data(
        http_json(
            "GET",
            f"{settings.ferriskey_base_url}/realms/{settings.realm}/users",
            token=token,
        )
    )
    for user in users:
        if user.get("username") == username:
            return user
    return None


def reset_user_password(settings: Settings, token: str, user_id: str, password: str) -> None:
    http_json(
        "PUT",
        f"{settings.ferriskey_base_url}/realms/{settings.realm}/users/{user_id}/reset-password",
        token=token,
        json_body={
            "credential_type": "password",
            "temporary": False,
            "value": password,
        },
        expected=(200,),
    )


def assign_realm_roles(
    settings: Settings, token: str, user_id: str, roles: list[dict[str, Any]]
) -> None:
    assigned = response_data(
        http_json(
            "GET",
            f"{settings.ferriskey_base_url}/realms/{settings.realm}/users/{user_id}/roles",
            token=token,
        )
    )
    assigned_ids = {role.get("id") for role in assigned}
    for role in roles:
        if role.get("id") in assigned_ids:
            continue
        http_json(
            "POST",
            f"{settings.ferriskey_base_url}/realms/{settings.realm}/users/{user_id}/roles/{role['id']}",
            token=token,
            expected=(200,),
        )


def sync_postgres(settings: Settings, alon: dict[str, Any], alice: dict[str, Any]) -> None:
    roles_values = ",\n".join(f"({sql_quote(role)})" for role in ROLE_NAMES)
    sql = f"""
WITH tenant AS (
    INSERT INTO tenants (name, slug, metadata)
    VALUES ('Bibi Work', {sql_quote(settings.tenant_slug)}, '{{"bootstrap": "ferriskey"}}'::jsonb)
    ON CONFLICT (slug) DO UPDATE SET updated_at = CURRENT_TIMESTAMP
    RETURNING id
), alon_user AS (
    INSERT INTO platform_users (
        tenant_id, ferriskey_subject, username, email, display_name, status
    )
    SELECT id, {sql_quote(alon["id"])}, 'alon', {sql_quote(alon.get("email"))},
           {sql_quote(display_name(alon, "Alon"))}, 'active'
    FROM tenant
    ON CONFLICT (tenant_id, ferriskey_subject)
    DO UPDATE SET
        username = EXCLUDED.username,
        email = EXCLUDED.email,
        display_name = EXCLUDED.display_name,
        status = 'active',
        updated_at = CURRENT_TIMESTAMP
    RETURNING id, tenant_id
), alice_user AS (
    INSERT INTO platform_users (
        tenant_id, ferriskey_subject, username, email, display_name, status
    )
    SELECT id, {sql_quote(alice["id"])}, 'alice', {sql_quote(alice.get("email"))},
           {sql_quote(display_name(alice, "Alice"))}, 'active'
    FROM tenant
    ON CONFLICT (tenant_id, ferriskey_subject)
    DO UPDATE SET
        username = EXCLUDED.username,
        email = EXCLUDED.email,
        display_name = EXCLUDED.display_name,
        status = 'active',
        updated_at = CURRENT_TIMESTAMP
    RETURNING id, tenant_id
), memberships AS (
    INSERT INTO user_tenant_memberships (tenant_id, user_id, role)
    SELECT tenant_id, id, 'admin' FROM alon_user
    UNION ALL
    SELECT tenant_id, id, 'member' FROM alice_user
    ON CONFLICT (tenant_id, user_id) DO UPDATE SET role = EXCLUDED.role
), role_names(role_name) AS (
    VALUES
    {roles_values}
), role_projection AS (
    INSERT INTO ferriskey_role_projection (tenant_id, role_name, role_kind)
    SELECT tenant.id, role_names.role_name, 'realm'
    FROM tenant CROSS JOIN role_names
    ON CONFLICT (tenant_id, role_name)
    DO UPDATE SET last_seen_at = CURRENT_TIMESTAMP
), tenant_relations AS (
    INSERT INTO resource_relations (
        tenant_id, resource_type, resource_id, relation, subject_type, subject_id
    )
    SELECT tenant.id, 'tenant', tenant.id::text, 'admin', 'user', alon_user.id::text
    FROM tenant CROSS JOIN alon_user
    UNION ALL
    SELECT tenant.id, 'tenant', tenant.id::text, 'member', 'user', alice_user.id::text
    FROM tenant CROSS JOIN alice_user
    ON CONFLICT (tenant_id, resource_type, resource_id, relation, subject_type, subject_id)
    DO UPDATE SET disabled_at = NULL
), tenant_policy AS (
    INSERT INTO resource_policy_bindings (
        tenant_id, resource_type, resource_id, action, subject_type, subject_id, effect, risk_level
    )
    SELECT tenant.id, 'tenant', tenant.id::text, '*', 'relation', 'admin', 'allow', 'low'
    FROM tenant
    WHERE NOT EXISTS (
        SELECT 1
        FROM resource_policy_bindings binding
        WHERE binding.tenant_id = tenant.id
          AND binding.resource_type = 'tenant'
          AND binding.resource_id = tenant.id::text
          AND binding.action = '*'
          AND binding.subject_type = 'relation'
          AND binding.subject_id = 'admin'
          AND binding.effect = 'allow'
          AND binding.disabled_at IS NULL
    )
)
SELECT 'bootstrap synced' AS status;
"""
    subprocess.run(["psql", settings.database_url], input=sql, text=True, check=True)


def display_name(user: dict[str, Any], fallback: str) -> str:
    return (
        user.get("firstName")
        or user.get("firstname")
        or user.get("name")
        or user.get("username")
        or fallback
    )


def http_maybe_json(
    settings: Settings, token: str, method: str, path: str
) -> dict[str, Any] | list[Any] | None:
    try:
        return http_json(method, f"{settings.ferriskey_base_url}{path}", token=token)
    except urllib.error.HTTPError as exc:
        if exc.code == 404:
            return None
        raise
    except RuntimeError as exc:
        if " failed with 404:" in str(exc):
            return None
        raise


def response_data(response: Any) -> list[dict[str, Any]]:
    data = response.get("data") if isinstance(response, dict) else response
    if isinstance(data, list):
        return data
    return []


def http_json(
    method: str,
    url: str,
    *,
    token: str | None = None,
    json_body: Any | None = None,
    data: bytes | None = None,
    headers: dict[str, str] | None = None,
    expected: tuple[int, ...] = (200,),
) -> Any:
    request_headers = dict(headers or {})
    if token:
        request_headers["authorization"] = f"Bearer {token}"
    if json_body is not None:
        data = json.dumps(json_body).encode()
        request_headers["content-type"] = "application/json"
    request = urllib.request.Request(url, data=data, method=method, headers=request_headers)
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            if response.status not in expected:
                raise RuntimeError(f"{method} {url} returned {response.status}")
            body = response.read()
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode(errors="replace")
        raise RuntimeError(f"{method} {url} failed with {exc.code}: {detail}") from exc
    if not body:
        return {}
    return json.loads(body)


def sql_quote(value: Any | None) -> str:
    if value is None:
        return "NULL"
    return "'" + str(value).replace("'", "''") + "'"


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        print(f"bootstrap failed: {exc}", file=sys.stderr)
        raise
