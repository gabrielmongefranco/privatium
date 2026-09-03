-- Project:  Privatium™  |  File: crates/privatium-core/src/store/sys.sql
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-01  |  Modified: 2026-09-03
-- Summary:  The framework's own schema.sql (spec/data-dictionary.md §3). `_sys` is an app
--           and is materialized by exactly the machinery any app gets; this is the file it
--           would have shipped if it had a folder.

-- Written in `main` and materialized into the DuckDB schema `sys`
-- (spec/data-dictionary.md §1, §3 preamble). Introspection reads `main`, so the schema
-- qualifier belongs to the materializer, not here.

-- Every table below is REPLICATED — event-sourced into data/_sys/log/ and synced.
--
-- Four tables of §3 are deliberately absent, and their absence is the design rather than
-- an omission. §3.3 `sys_pairing`, §3.7 `sys_peer`, §3.7b `sys_endpoint` and §3.8
-- `sys_sync_state` are marked **local store only**: they live in `local/state.jsonl`, are
-- never synced and are not in a backup (§1), so there are no events in data/_sys/ for them
-- to be replayed from. Adding them here would create four permanently empty tables that
-- look like a sync bug.
--
-- §3.11 `sys_migration` is absent because it is reserved and not implemented in pv/1.

CREATE TABLE sys_node (
    id              VARCHAR PRIMARY KEY,   -- Node ID, not a ULID (§3.1)
    display_name    VARCHAR,
    pubkey          VARCHAR,
    created_at      TIMESTAMPTZ,
    protocol        VARCHAR,
    build           VARCHAR,
    cluster_id      VARCHAR,
    cert            VARCHAR,
    cert_expires_at TIMESTAMPTZ
);

CREATE TABLE sys_cluster (
    id         VARCHAR PRIMARY KEY,        -- Cluster ID (§3.1b)
    pubkey     VARCHAR,                    -- public key only; never the private key
    pkarr_name VARCHAR,
    created_at TIMESTAMPTZ,
    created_by VARCHAR,
    label      VARCHAR
);

CREATE TABLE sys_node_revocation (
    id         VARCHAR PRIMARY KEY,        -- the revoked Node ID (§3.1c)
    revoked_at TIMESTAMPTZ,
    revoked_by VARCHAR,
    reason     VARCHAR
);

CREATE TABLE sys_device (
    id             VARCHAR PRIMARY KEY,    -- device Node ID, not a ULID (§3.2)
    label          VARCHAR,
    kind           VARCHAR,                -- browser | desktop | mobile | node
    replica        BOOLEAN,
    ed25519_pub    VARCHAR,
    x25519_pub     VARCHAR,
    paired_at      TIMESTAMPTZ,
    paired_via     VARCHAR,
    last_seen_at   TIMESTAMPTZ,
    user_agent     VARCHAR,
    revoked_at     TIMESTAMPTZ,
    revoked_reason VARCHAR
);

CREATE TABLE sys_app (
    id            VARCHAR PRIMARY KEY,     -- the slug; slugs are the natural key (§3.4)
    title         VARCHAR,
    version       VARCHAR,
    api           INTEGER,
    tier          VARCHAR,
    icon          VARCHAR,
    source        VARCHAR,
    enabled       BOOLEAN,
    nav_order     INTEGER,
    installed_at  TIMESTAMPTZ,
    updated_at    TIMESTAMPTZ,
    schema_hash   VARCHAR,
    manifest_hash VARCHAR,
    advertise     BOOLEAN,
    permissions   VARCHAR,
    last_error    VARCHAR
);

CREATE TABLE sys_app_grant (
    id         VARCHAR PRIMARY KEY,        -- ULID (§3.5)
    device_id  VARCHAR,
    app_id     VARCHAR,
    access     VARCHAR,
    granted_at TIMESTAMPTZ
);

CREATE TABLE sys_setting (
    id         VARCHAR PRIMARY KEY,        -- the dotted setting key (§3.6)
    value      VARCHAR,
    updated_at TIMESTAMPTZ
);

CREATE TABLE sys_snapshot (
    id          VARCHAR PRIMARY KEY,       -- snapshot ID (§3.9)
    app_id      VARCHAR,
    created_at  TIMESTAMPTZ,
    hi_lam      BIGINT,
    row_counts  VARCHAR,
    bytes       BIGINT,
    created_by  VARCHAR,
    verified_at TIMESTAMPTZ
);

-- `detail` is VARCHAR holding JSON, not a JSON column. §3.10 types it VARCHAR and §2.1
-- encodes VARCHAR as a string, so a row carries "detail":"{\"dev\":\"…\"}" — a string that
-- contains JSON. Typing it JSON here would diverge from the dictionary on the one table
-- whose whole job is to be trustworthy. §3.9's `row_counts` is the same shape.
--
-- `at` is quoted because DuckDB reserves it for `AT TIME ZONE`. §3.10 names the column
-- `at`, and §5's list of column names to avoid — date, time, order, group, user — does not
-- include it, so the dictionary is right and the identifier simply needs quoting. Of every
-- name in this file it is the only one DuckDB objects to.
CREATE TABLE sys_audit (
    id       VARCHAR PRIMARY KEY,          -- ULID (§3.10)
    "at"     TIMESTAMPTZ,
    kind     VARCHAR,
    actor    VARCHAR,
    subject  VARCHAR,
    detail   VARCHAR,
    severity VARCHAR
);

-- Framework views (spec/data-dictionary.md §4). Apps MAY read these; they MUST NOT write
-- to sys tables.
--
-- `sys.v_health` is the fourth and is NOT here. It reads `pv.health`, a table the
-- framework maintains in cache/_sys.duckdb with each app's restore tier (§5.3), and this
-- file is parsed in an in-memory instance that has no such table for a view to bind
-- against. `Store::ensure_health` creates both, after every rebuild, beside the views
-- below. The restore tier is node-local by nature — a fact about this node's cache — which
-- is why it is a cache table joined to the replicated `sys_snapshot` rather than an event.

-- `last_error` is carried so the launcher can show an enabled app whose folder is missing
-- as unavailable rather than pretending it is gone (§3.4, rules). §4 does not enumerate the
-- view's columns, so this is the framework's call and not a dictionary change.
CREATE VIEW v_app_nav AS
    SELECT id, title, icon, nav_order, last_error
    FROM sys_app
    WHERE enabled
    ORDER BY nav_order, title;

CREATE VIEW v_device_active AS
    SELECT * FROM sys_device WHERE revoked_at IS NULL;

CREATE VIEW v_audit_recent AS
    SELECT * FROM sys_audit ORDER BY "at" DESC LIMIT 200;
