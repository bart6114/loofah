ALTER TABLE sync_devices ADD COLUMN name TEXT CHECK (name IS NULL OR length(name) BETWEEN 1 AND 80);
