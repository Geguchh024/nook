-- Usage counts from Nook 0.2.1 on. Apply once to the live database:
--   npx wrangler@4.153.0 d1 execute nook-stats --remote --file migrations/0002_usage.sql
ALTER TABLE pings ADD COLUMN opens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pings ADD COLUMN minutes INTEGER NOT NULL DEFAULT 0;
-- 1 when the ping carried usage counts (older versions send none).
ALTER TABLE pings ADD COLUMN reported INTEGER NOT NULL DEFAULT 0;

-- Uses of each built-in tool per day, summed over all installs. Holds no
-- install id, so it can't be tied back to anyone.
CREATE TABLE IF NOT EXISTS tools (
  day  TEXT NOT NULL,
  tool TEXT NOT NULL,
  uses INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (day, tool)
);
