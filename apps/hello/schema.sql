-- This file is part of Privatium
-- apps/hello/schema.sql
-- Author(s): Gabriel Mongefranco
-- Created: 2026-08-28
-- Last Modified: 2026-09-03
-- Summary: One table, one column. Derived from the event log on every start.
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

CREATE TABLE profile (
    id           VARCHAR PRIMARY KEY,   -- ULID, minted by the framework
    display_name VARCHAR NOT NULL
);

-- profile: the one person using this node. display_name is what the app should call you.
