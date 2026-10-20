-- Retirement hides an identity from the active fleet without discarding history. A fresh
-- observation clears the marker and returns the device to the active inventory.
CREATE TABLE IF NOT EXISTS retired_devices (
    device_id TEXT PRIMARY KEY,
    retired_at REAL NOT NULL
);
