-- Project:  Privatium™  |  File: apps/animals/schema.sql
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-08-28  |  Modified: 2026-08-28
-- Summary:  A binary decision tree in one table. Leaves are animals, branches
--           are yes/no questions.

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

COMMENT ON TABLE node   IS 'The decision tree. Grows by one branch per wrong guess.';
COMMENT ON TABLE cursor IS 'Single row, id = "cursor". Survives reloads and device switches.';
