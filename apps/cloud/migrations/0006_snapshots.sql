CREATE TABLE sync_snapshots (
  id TEXT PRIMARY KEY,
  vault_id TEXT NOT NULL REFERENCES sync_accounts(vault_id),
  generation TEXT NOT NULL,
  watermark INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE INDEX sync_snapshots_expiry ON sync_snapshots(expires_at);
CREATE TABLE sync_snapshot_items (
  snapshot TEXT NOT NULL REFERENCES sync_snapshots(id) ON DELETE CASCADE,
  vault_id TEXT NOT NULL,
  entity TEXT NOT NULL,
  revision TEXT NOT NULL,
  PRIMARY KEY (snapshot, entity, revision),
  FOREIGN KEY (vault_id, revision) REFERENCES sync_revisions(vault_id, id)
);
CREATE INDEX sync_snapshot_pins ON sync_snapshot_items(vault_id, revision);

CREATE TRIGGER sync_snapshot_limit BEFORE INSERT ON sync_snapshots BEGIN
  SELECT RAISE(ABORT, 'snapshot unavailable') WHERE NOT EXISTS (
    SELECT 1 FROM sync_accounts WHERE vault_id = NEW.vault_id AND recovery_generation = NEW.generation AND active = 1);
  SELECT RAISE(ABORT, 'too many snapshots') WHERE (SELECT count(*) FROM sync_snapshots WHERE vault_id = NEW.vault_id) >= 5;
END;
CREATE TRIGGER sync_snapshot_capture AFTER INSERT ON sync_snapshots BEGIN
  INSERT INTO sync_snapshot_items (snapshot, vault_id, entity, revision)
    SELECT NEW.id, vault_id, entity, revision FROM sync_heads WHERE vault_id = NEW.vault_id;
  INSERT INTO sync_snapshot_items (snapshot, vault_id, entity, revision)
    SELECT NEW.id, vault_id, entity, id FROM sync_revisions WHERE vault_id = NEW.vault_id AND pinned = 1;
END;

CREATE TABLE sync_resolutions (
  vault_id TEXT NOT NULL,
  revision TEXT NOT NULL,
  conflict TEXT NOT NULL,
  PRIMARY KEY (vault_id, conflict),
  FOREIGN KEY (vault_id, revision) REFERENCES sync_revisions(vault_id, id) ON DELETE CASCADE,
  FOREIGN KEY (vault_id, conflict) REFERENCES sync_revisions(vault_id, id) ON DELETE CASCADE
);
CREATE TRIGGER sync_resolve_validate BEFORE INSERT ON sync_resolutions BEGIN
  SELECT RAISE(ABORT, 'invalid conflict resolution') WHERE NOT EXISTS (
    SELECT 1 FROM sync_revisions r JOIN sync_revisions c ON r.vault_id = c.vault_id AND r.entity = c.entity
    WHERE r.vault_id = NEW.vault_id AND r.id = NEW.revision AND r.operation = 'resolve' AND c.id = NEW.conflict AND c.pinned = 1);
END;
CREATE TRIGGER sync_resolve_release AFTER INSERT ON sync_resolutions BEGIN
  UPDATE sync_revisions SET pinned = 0, superseded_at = (
    SELECT created_at FROM sync_revisions WHERE vault_id = NEW.vault_id AND id = NEW.revision)
    WHERE vault_id = NEW.vault_id AND id = NEW.conflict;
END;
