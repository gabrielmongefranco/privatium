-- Project:  Privatium™  |  File: apps/_lint/fail/PV107/pv107bad/schema.sql
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV107 fail: an INSERT after the declaration; rows arrive by append.

CREATE TABLE note (
    id   VARCHAR PRIMARY KEY,
    text VARCHAR NOT NULL
);

INSERT INTO note (id, text) VALUES ('seed', 'a row that should have been an event');
