# Tenancy: model, roles, and enforcement

Status: design (in progress). Tracks FOLLOWUPS section 14.

## 1. The model: scope x level

Access has two orthogonal dimensions, and we encode them separately (the old flat role
enum conflated them):

- **Scope** = which data you can touch. Encoded as the user's `tenant_id`:
  `NULL` = system (all tenants, platform-wide), `Some(x)` = only tenant x.
- **Level** = what you can do within that scope. Encoded as the `role`:
  `Admin` / `Operator` / `Viewer`. (The old `Tenant` role is removed, it was a scope
  pretending to be a level.)

Effective access = scope x level:

| user | tenant_id | role | can do |
|---|---|---|---|
| System Admin | NULL | Admin | everything, all tenants, manage tenants + users (platform owner) |
| System Operator | NULL | Operator | run/cancel/manage jobs across all tenants (platform support) |
| System Viewer | NULL | Viewer | read-only across all tenants |
| Tenant Admin | X | Admin | manage tenant X fully (its users, jobs, secrets, configs), not other tenants, not the platform |
| Tenant Operator | X | Operator | create/run/cancel jobs + manage secrets/configs in X, not X's users/settings |
| Tenant Viewer | X | Viewer | read-only within X |

## 2. Resource ownership

- A `tenants` table (`id`, `name`, `created_at`) with a seeded **default** tenant
  (a fixed well-known id).
- **Tenant-owned (NOT NULL `tenant_id`, defaults to the default tenant):** jobs, secrets,
  shared configs (pgsql/mysql). Runs derive their tenant from their job. Env vars inherit
  from their job. Every such row always lives in exactly one tenant.
- **Users (`tenant_id` nullable):** `NULL` = system-scope, otherwise the user's tenant.
- Name uniqueness becomes **per tenant** rather than global (jobs, secrets): two tenants
  may each have a `db-pass` secret or a `nightly` job. (Applied in the scoping increment.)

## 3. Enforcement

Scoping lives in the **store queries**, not only in the API, so it cannot be bypassed:

- The API derives the caller's scope from their JWT (their `tenant_id` + role).
- List/get for jobs/runs/secrets/configs take a **tenant scope**: `None` for a system
  caller (sees all), `Some(x)` for a tenant caller (restricted to x). Writes stamp the
  caller's tenant.
- **Secret isolation (SECRETS.md I7):** `resolve_secret` checks the requesting job's tenant
  against the secret's tenant and refuses a mismatch (fail closed). A job may only resolve
  secrets in its own tenant.
- A tenant caller cannot escalate scope (cannot pass a tenant other than their own).

## 4. Bootstrap / migration

Pre-alpha, so there is no production data to migrate. The schema seeds the default tenant.
The seeded admin becomes a **System Admin** (`tenant_id` NULL) so it can manage the
platform. Any pre-existing tenant-owned rows take the default tenant via the column default.

## 5. Increments

1. **Data model (this step):** `tenants` table + seeded default tenant + `tenant_id`
   columns (users nullable, jobs/secrets/configs NOT NULL default-tenant) + remove the
   `Tenant` role + `TenantStore` (create/get/list) + `users.tenant_id` read/write +
   conformance. Rows get the default tenant via the column default for now (no scoping yet).
2. **Scoping + API (done):** `create_job` stamps the caller's tenant, `list_jobs`/`get_job`/
   `list_recent_runs` filter by scope; secrets are per-tenant (increment 2a). The JWT
   `Claims` carry `tenant_id` (encoded at login); handlers derive scope (`None` = system,
   `Some(t)` = tenant) and gate job mutations via a scoped `get_job`. Conformance
   `tenant::jobs_and_runs_scoped`. Remaining gaps: `cancel_run` is keyed by run id and not
   yet tenant-scoped (needs the run's tenant), and the create-user API does not yet set the
   new user's tenant (passes `None`); both are increment-4/UI work.
3. **Secret isolation (I7) (done):** secrets are unique per tenant; the worker resolves a
   run's secrets within its job's tenant (`job_tenant` + `get_secret_by_name(tenant, name)`),
   fail closed. Conformance `secrets::isolated_per_tenant`.
4. **UI:** tenant management (system admin), a tenant context/picker, and tenant-aware
   listings. Folds into the secrets/UI work.

## 6. Open decisions

- Whether a Tenant Admin manages its own users (likely yes) and how user invitation works.
- Per-tenant quotas/limits (out of scope for now).
