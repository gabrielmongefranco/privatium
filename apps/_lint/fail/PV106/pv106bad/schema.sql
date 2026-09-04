-- Project:  Privatium™  |  File: apps/_lint/fail/PV106/pv106bad/schema.sql
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV106 fail: id INTEGER, and the key is elsewhere.

CREATE TABLE note (
    id   INTEGER,
    text VARCHAR PRIMARY KEY
);
