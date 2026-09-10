-- This file is part of Privatium
-- apps/animals/schema.sql
-- Author(s): Gabriel Mongefranco
-- Created: 2026-08-28
-- Last Modified: 2026-09-03
-- Summary: A binary decision tree in one table. Leaves are animals, branches are yes/no questions.
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

CREATE TABLE node (
    id     VARCHAR PRIMARY KEY,   -- ULID
    kind   VARCHAR NOT NULL,      -- 'q' = question (branch), 'a' = animal (leaf)
    text   VARCHAR NOT NULL,      -- the question, or the animal's name
    yes_id VARCHAR,               -- child when yes; NULL for leaves
    no_id  VARCHAR,               -- child when no;  NULL for leaves
    CHECK (kind IN ('q', 'a')),
    CHECK ((kind = 'a' AND yes_id IS NULL AND no_id IS NULL)
        OR (kind = 'q' AND yes_id IS NOT NULL AND no_id IS NOT NULL))
);

-- Where the current round is. Replicated, so you can start a game on the laptop
-- and finish it on the phone.
CREATE TABLE cursor (
    id      VARCHAR PRIMARY KEY,
    node_id VARCHAR NOT NULL,
    started TIMESTAMPTZ NOT NULL
);

-- node:   the decision tree; grows by one branch per wrong guess.
-- cursor: single row, id = "cursor"; survives reloads and device switches.
