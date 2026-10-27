-- One row per install per day it was opened (UTC). `first` marks the day
-- the install was created. No IPs, names or anything else is stored.
-- A fresh database takes this file; an existing one takes migrations/.
CREATE TABLE IF NOT EXISTS pings (
  id       TEXT NOT NULL,
  day      TEXT NOT NULL,
  version  TEXT NOT NULL,
  os       TEXT NOT NULL DEFAULT '',
  first    INTEGER NOT NULL DEFAULT 0,
  -- Panel opens and minutes open since the install's previous ping.
  opens    INTEGER NOT NULL DEFAULT 0,
  minutes  INTEGER NOT NULL DEFAULT 0,
  -- 1 when the ping carried usage counts (older versions send none).
  reported INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (id, day)
);
CREATE INDEX IF NOT EXISTS pings_day ON pings (day);

-- Uses of each built-in tool per day, summed over all installs. Holds no
-- install id, so it can't be tied back to anyone.
CREATE TABLE IF NOT EXISTS tools (
  day  TEXT NOT NULL,
  tool TEXT NOT NULL,
  uses INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (day, tool)
);
