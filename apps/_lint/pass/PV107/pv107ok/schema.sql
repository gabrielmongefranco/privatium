-- Project:  Privatium™  |  File: apps/_lint/pass/PV107/pv107ok/schema.sql
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV107 pass: three kinds of declaration; a comment may say INSERT.

-- A comment is free to mention INSERT, UPDATE or DELETE.
CREATE TABLE note (
    id      VARCHAR PRIMARY KEY,
    text    VARCHAR NOT NULL,
    made_on DATE
);

CREATE VIEW v_recent AS
    SELECT id, text FROM note WHERE made_on >= date('now', '-30 days');

CREATE INDEX note_made_on ON note (made_on);
