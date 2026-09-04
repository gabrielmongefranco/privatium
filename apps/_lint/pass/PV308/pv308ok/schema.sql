-- Project:  Privatium™  |  File: apps/_lint/pass/PV308/pv308ok/schema.sql
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  A DECIMAL and a DATE column.

CREATE TABLE fill (id VARCHAR PRIMARY KEY, copay DECIMAL(18,2), due_on DATE);
