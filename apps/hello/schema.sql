-- Project:  Privatium™  |  File: apps/hello/schema.sql
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-08-28  |  Modified: 2026-09-03
-- Summary:  One table, one column. Derived from the event log on every start.

CREATE TABLE profile (
    id           VARCHAR PRIMARY KEY,   -- ULID, minted by the framework
    display_name VARCHAR NOT NULL
);

-- profile: the one person using this node. display_name is what the app should call you.
