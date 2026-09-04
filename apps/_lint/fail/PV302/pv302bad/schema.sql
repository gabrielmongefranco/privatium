-- Project:  Privatium™  |  File: apps/_lint/fail/PV302/pv302bad/schema.sql
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-05  |  Modified: 2026-09-05
-- Summary:  A DECIMAL column.

CREATE TABLE fill (id VARCHAR PRIMARY KEY, copay DECIMAL(18,2));
