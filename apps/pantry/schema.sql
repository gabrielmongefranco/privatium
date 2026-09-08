-- Project:  Privatium™  |  File: apps/pantry/schema.sql
-- Authors:  Gabriel Mongefranco (@gabrielmongefranco)
-- Created:  2026-09-07  |  Modified: 2026-09-07
-- Summary:  Shelves, batches, and one row per recorded change to a batch. A balance is
--           never stored: it is decimal_sum() over the changes, so two devices that both
--           take the last portion converge on a negative number the app can report rather
--           than on a lost write. See main README.md for full license information.

CREATE TABLE shelf (                 -- one labelled space: a freezer drawer, a pantry shelf
    id   VARCHAR PRIMARY KEY,        -- ULID
    name VARCHAR NOT NULL,
    sort BIGINT  NOT NULL            -- where it sits in the map; ties break on name
);

CREATE TABLE batch (                 -- one stored batch, live for as long as the app is
    id         VARCHAR PRIMARY KEY,
    name       VARCHAR NOT NULL,
    icon       VARCHAR NOT NULL,     -- a vendored Bootstrap Icons name (docs/icons.md)
    unit       VARCHAR NOT NULL,     -- chosen when the batch is added, never changed
    shelf_id   VARCHAR NOT NULL,
    stored_on  DATE    NOT NULL,     -- frozen on, or shelved on
    expires_on DATE                  -- NULL when nothing on the package says
);

CREATE TABLE quantity_change (       -- one recorded change to one batch
    id       VARCHAR       PRIMARY KEY,
    batch_id VARCHAR       NOT NULL,
    amount   DECIMAL(18,3) NOT NULL, -- negative takes out; positive stocks or returns
    reason   VARCHAR       NOT NULL, -- 'stocked' | 'taken' | 'returned'
    of_id    VARCHAR,                -- the withdrawal a return refers to; else NULL
    at       TIMESTAMPTZ   NOT NULL, -- the writing device's clock, in UTC
    CHECK (reason IN ('stocked', 'taken', 'returned')),
    CHECK ((reason = 'returned') = (of_id IS NOT NULL))
);

CREATE INDEX ix_change_batch ON quantity_change (batch_id);
CREATE INDEX ix_change_of    ON quantity_change (of_id);

-- grain: one row per shelf, with a count of the batches on it that still hold something.
CREATE VIEW v_shelf AS
SELECT s.id,
       s.name,
       s.sort,
       (SELECT count(*)
          FROM batch b
         WHERE b.shelf_id = s.id
           AND decimal_cmp((SELECT decimal_sum(c.amount)
                              FROM quantity_change c
                             WHERE c.batch_id = b.id), '0') <> 0) AS batches
  FROM shelf s
 ORDER BY s.sort, s.name;

-- grain: one row per batch on the shelf $shelf whose balance is not zero. The balance is
-- the sum of the changes, never a column; a batch that reaches zero leaves this view and
-- keeps its history.
CREATE VIEW v_batch AS
SELECT b.id,
       b.name,
       b.icon,
       b.unit,
       b.shelf_id,
       b.stored_on,
       CAST(julianday('now') - julianday(b.stored_on) AS INTEGER) AS days_in,
       b.expires_on,
       CASE WHEN b.expires_on IS NULL              THEN 'unknown'
            WHEN b.expires_on <  date('now')       THEN 'past'
            WHEN b.expires_on <= date('now', '+7 days') THEN 'soon'
            ELSE 'fresh'
       END AS expiry,
       decimal_sum(c.amount) AS balance
  FROM batch b
  JOIN quantity_change c ON c.batch_id = b.id
 WHERE b.shelf_id = $shelf
 GROUP BY b.id, b.name, b.icon, b.unit, b.shelf_id, b.stored_on, b.expires_on
HAVING decimal_cmp(decimal_sum(c.amount), '0') <> 0
 ORDER BY b.name;

-- grain: one row per withdrawal made at or after $since, with how much of it has come
-- back. A withdrawal is never edited; a return is another row pointing at it.
CREATE VIEW v_out AS
SELECT o.id,
       o.batch_id,
       o.at,
       o.taken,
       o.returned,
       decimal_sub(o.taken, o.returned) AS still_out,
       b.name AS batch_name,
       b.icon,
       b.unit,
       s.name AS shelf_name
  FROM (SELECT t.id,
               t.batch_id,
               t.at,
               decimal_sub('0', t.amount) AS taken,
               coalesce((SELECT decimal_sum(r.amount)
                           FROM quantity_change r
                          WHERE r.of_id = t.id), '0') AS returned
          FROM quantity_change t
         WHERE t.reason = 'taken'
           AND t.at >= $since) o
  JOIN batch b ON b.id = o.batch_id
  JOIN shelf s ON s.id = b.shelf_id
 ORDER BY o.at DESC;

-- grain: one row per unit of measure, over every batch on every shelf.
CREATE VIEW v_stock_by_unit AS
SELECT b.unit,
       decimal_sum(c.amount)  AS balance,
       count(DISTINCT b.id)   AS batches
  FROM batch b
  JOIN quantity_change c ON c.batch_id = b.id
 GROUP BY b.unit
 ORDER BY b.unit;

-- grain: one row per batch whose balance is below zero — two devices took the same last
-- portion. The recorded number is shown, not clamped.
CREATE VIEW v_check_batch AS
SELECT b.id,
       b.name,
       b.unit,
       s.name AS shelf_name,
       decimal_sum(c.amount) AS balance
  FROM batch b
  JOIN quantity_change c ON c.batch_id = b.id
  JOIN shelf s ON s.id = b.shelf_id
 GROUP BY b.id, b.name, b.unit, s.name
HAVING decimal_cmp(decimal_sum(c.amount), '0') < 0
 ORDER BY b.name;

-- grain: one row per withdrawal that has had more put back than was taken out.
CREATE VIEW v_check_out AS
SELECT o.id,
       o.batch_id,
       o.taken,
       o.returned,
       b.name AS batch_name,
       b.unit
  FROM (SELECT t.id,
               t.batch_id,
               decimal_sub('0', t.amount) AS taken,
               coalesce((SELECT decimal_sum(r.amount)
                           FROM quantity_change r
                          WHERE r.of_id = t.id), '0') AS returned
          FROM quantity_change t
         WHERE t.reason = 'taken') o
  JOIN batch b ON b.id = o.batch_id
 WHERE decimal_cmp(o.returned, o.taken) > 0
 ORDER BY o.id;

-- grain: one row per batch that still holds something and carries a date within $days, or
-- is already past it. A date is compared with SQLite's own modifier — expires_on + $days
-- would be integer arithmetic on text, which is the mistake PV308 exists to catch.
CREATE VIEW v_expiring AS
SELECT b.id,
       b.name,
       b.icon,
       b.unit,
       b.expires_on,
       s.name AS shelf_name,
       CASE WHEN b.expires_on < date('now') THEN 'past' ELSE 'soon' END AS expiry,
       decimal_sum(c.amount) AS balance
  FROM batch b
  JOIN quantity_change c ON c.batch_id = b.id
  JOIN shelf s ON s.id = b.shelf_id
 WHERE b.expires_on IS NOT NULL
   AND b.expires_on <= date('now', '+' || $days || ' days')
 GROUP BY b.id, b.name, b.icon, b.unit, b.expires_on, s.name
HAVING decimal_cmp(decimal_sum(c.amount), '0') <> 0
 ORDER BY b.expires_on, b.name;
