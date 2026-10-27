-- One row per install per day it was opened (UTC). `first` marks the day
-- the install was created. No IPs, names or anything else is stored.
CREATE TABLE IF NOT EXISTS pings (
  id      TEXT NOT NULL,
  day     TEXT NOT NULL,
  version TEXT NOT NULL,
  os      TEXT NOT NULL DEFAULT '',
  first   INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (id, day)
);
CREATE INDEX IF NOT EXISTS pings_day ON pings (day);
