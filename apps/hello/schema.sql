-- Project:  Privatium™  |  File: apps/hello/schema.sql
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-08-28  |  Modified: 2026-08-28
-- Summary:  One table, one column. Derived from the event log on every start.

CREATE TABLE profile (
    id           VARCHAR PRIMARY KEY,   -- ULID, minted by the framework
    display_name VARCHAR NOT NULL
);

COMMENT ON TABLE  profile              IS 'The one person using this node.';
COMMENT ON COLUMN profile.display_name IS 'What the app should call you.';
