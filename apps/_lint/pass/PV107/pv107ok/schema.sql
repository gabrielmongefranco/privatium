-- This file is part of Privatium
-- apps/_lint/pass/PV107/pv107ok/schema.sql
-- Author(s): Gabriel Mongefranco
-- Created: 2026-09-05
-- Last Modified: 2026-09-05
-- Summary: PV107 pass: three kinds of declaration; a comment may say INSERT.
-- Notes: See README file for documentation and full license information.
--
-- Copyright © 2026 Gabriel Mongefranco
--
-- This program is free software: you can redistribute it and/or modify
-- it under the terms of the GNU General Public License as published by
-- the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
--
-- This program is distributed in the hope that it will be useful,
-- but WITHOUT ANY WARRANTY; without even the implied warranty of
-- MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
-- GNU General Public License for more details.
--
-- You should have received a copy of the GNU General Public License along
-- with this program. If not, see <https://www.gnu.org/licenses/>.

-- A comment is free to mention INSERT, UPDATE or DELETE.
CREATE TABLE note (
    id      VARCHAR PRIMARY KEY,
    text    VARCHAR NOT NULL,
    made_on DATE
);

CREATE VIEW v_recent AS
    SELECT id, text FROM note WHERE made_on >= date('now', '-30 days');

CREATE INDEX note_made_on ON note (made_on);
