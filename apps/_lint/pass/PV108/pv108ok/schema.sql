-- Project:  Privatium™  |  File: apps/_lint/pass/PV108/pv108ok/schema.sql
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV108 pass: id is the one key; a plain index speeds a lookup and promises nothing.

CREATE TABLE note (
    id      VARCHAR PRIMARY KEY,
    code    VARCHAR NOT NULL,
    made_on DATE
);

CREATE INDEX note_code ON note (code);
