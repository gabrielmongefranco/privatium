-- Project:  Privatium™  |  File: apps/_lint/fail/PV108/pv108bad/schema.sql
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV108 fail: a UNIQUE index. Two devices may both write the same code, and
--           the replay keeps both rows; the only key the log guarantees is id.

CREATE TABLE note (
    id      VARCHAR PRIMARY KEY,
    code    VARCHAR NOT NULL,
    made_on DATE
);

CREATE UNIQUE INDEX note_code ON note (code);
