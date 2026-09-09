-- Project:  Privatium™  |  File: apps/_lint/fail/PV305/pv305bad/schema.sql
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  PV305 fail: an outbox table with an acknowledgement column.
--           See main README.md for full license information.

CREATE TABLE outbox (id VARCHAR PRIMARY KEY, acked BOOLEAN);
