-- Project:  Privatium™  |  File: apps/_lint/pass/PV106/pv106ok/schema.sql
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  One table, keyed as spec/app-contract.md §4.5 requires.
--           See main README.md for full license information.

CREATE TABLE note (
    id   VARCHAR PRIMARY KEY,
    text VARCHAR NOT NULL
);
