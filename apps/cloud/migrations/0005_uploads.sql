ALTER TABLE sync_objects ADD COLUMN multipart_id TEXT;
CREATE TABLE sync_upload_parts (
  vault_id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  number INTEGER NOT NULL CHECK (number >= 1 AND number <= 10000),
  bytes INTEGER NOT NULL CHECK (bytes > 0 AND bytes <= 8388608),
  digest TEXT NOT NULL CHECK (length(digest) = 64),
  etag TEXT,
  PRIMARY KEY (vault_id, object_id, number),
  FOREIGN KEY (vault_id, object_id) REFERENCES sync_objects(vault_id, id)
);
CREATE TRIGGER sync_upload_part_reserve BEFORE INSERT ON sync_upload_parts BEGIN
  SELECT (CASE WHEN NOT EXISTS (SELECT 1 FROM sync_objects WHERE vault_id = NEW.vault_id AND id = NEW.object_id
    AND state = 'reserved' AND multipart_id IS NOT NULL) THEN RAISE(ABORT, 'upload unavailable') END);
  SELECT (CASE WHEN EXISTS (SELECT 1 FROM sync_upload_parts WHERE vault_id = NEW.vault_id AND object_id = NEW.object_id
    AND number = NEW.number AND bytes = NEW.bytes AND digest = NEW.digest) THEN RAISE(IGNORE) END);
END;
CREATE TRIGGER sync_upload_part_immutable BEFORE UPDATE ON sync_upload_parts
WHEN NEW.vault_id != OLD.vault_id OR NEW.object_id != OLD.object_id OR NEW.number != OLD.number
  OR NEW.bytes != OLD.bytes OR NEW.digest != OLD.digest
BEGIN SELECT RAISE(ABORT, 'immutable upload part'); END;
