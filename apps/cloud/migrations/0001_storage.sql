PRAGMA foreign_keys = ON;

-- Parenthesized CASE expressions avoid D1's remote trigger statement splitter.

CREATE TABLE sync_accounts (
  vault_id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL UNIQUE,
  active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
  quota_bytes INTEGER NOT NULL DEFAULT 25000000000 CHECK (quota_bytes > 0),
  used_bytes INTEGER NOT NULL DEFAULT 0 CHECK (used_bytes >= 0 AND used_bytes <= quota_bytes),
  control_limit INTEGER NOT NULL DEFAULT 134217728,
  control_bytes INTEGER NOT NULL DEFAULT 0 CHECK (control_bytes >= 0 AND control_bytes <= control_limit),
  recovery_generation TEXT NOT NULL
);

CREATE TABLE sync_objects (
  vault_id TEXT NOT NULL REFERENCES sync_accounts(vault_id),
  id TEXT NOT NULL,
  bytes INTEGER NOT NULL CHECK (bytes > 0),
  digest TEXT NOT NULL CHECK (length(digest) = 64),
  kind TEXT NOT NULL CHECK (kind IN ('file', 'manifest', 'control_manifest')),
  state TEXT NOT NULL CHECK (state IN ('reserved', 'uploaded', 'unreferenced', 'deleting', 'deleted')),
  refs INTEGER NOT NULL DEFAULT 0 CHECK (refs >= 0),
  charged INTEGER NOT NULL DEFAULT 1 CHECK (charged IN (0, 1)),
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  unreferenced_at INTEGER,
  PRIMARY KEY (vault_id, id),
  CHECK (kind != 'control_manifest' OR bytes <= 33554432),
  CHECK (refs = 0 OR (state = 'uploaded' AND charged = 1)),
  CHECK (state NOT IN ('unreferenced', 'deleting', 'deleted') OR (refs = 0 AND charged = 0))
);
CREATE INDEX sync_objects_cleanup ON sync_objects(state, unreferenced_at);
CREATE INDEX sync_objects_pending ON sync_objects(state, expires_at) WHERE refs = 0;

CREATE TRIGGER sync_objects_reserve BEFORE INSERT ON sync_objects BEGIN
  SELECT (CASE WHEN EXISTS (SELECT 1 FROM sync_objects WHERE vault_id = NEW.vault_id AND id = NEW.id
    AND bytes = NEW.bytes AND digest = NEW.digest AND kind = NEW.kind
    AND state IN ('reserved', 'uploaded', 'unreferenced')) THEN RAISE(IGNORE) END);
  SELECT (CASE WHEN NEW.state != 'reserved' OR NEW.refs != 0 OR NEW.charged != 1
    THEN RAISE(ABORT, 'invalid reservation') END);
  SELECT (CASE WHEN NOT EXISTS (SELECT 1 FROM sync_accounts WHERE vault_id = NEW.vault_id AND active = 1)
    THEN RAISE(ABORT, 'inactive vault') END);
  UPDATE sync_accounts SET
    used_bytes = used_bytes + (CASE WHEN NEW.kind != 'control_manifest' THEN NEW.bytes ELSE 0 END),
    control_bytes = control_bytes + (CASE WHEN NEW.kind = 'control_manifest' THEN NEW.bytes ELSE 0 END)
  WHERE vault_id = NEW.vault_id;
END;

CREATE TRIGGER sync_objects_charge BEFORE UPDATE OF charged ON sync_objects
WHEN NEW.charged != OLD.charged BEGIN
  UPDATE sync_accounts SET
    used_bytes = used_bytes + (CASE WHEN NEW.kind != 'control_manifest' THEN (NEW.charged - OLD.charged) * NEW.bytes ELSE 0 END),
    control_bytes = control_bytes + (CASE WHEN NEW.kind = 'control_manifest' THEN (NEW.charged - OLD.charged) * NEW.bytes ELSE 0 END)
  WHERE vault_id = NEW.vault_id;
END;

CREATE TRIGGER sync_objects_immutable BEFORE UPDATE ON sync_objects
WHEN NEW.vault_id != OLD.vault_id OR NEW.id != OLD.id OR NEW.bytes != OLD.bytes
  OR NEW.digest != OLD.digest OR NEW.kind != OLD.kind OR NEW.created_at != OLD.created_at
  OR (OLD.state IN ('deleting', 'deleted') AND NEW.state NOT IN ('deleting', 'deleted'))
BEGIN SELECT RAISE(ABORT, 'immutable object'); END;

CREATE TABLE sync_revisions (
  vault_id TEXT NOT NULL REFERENCES sync_accounts(vault_id),
  id TEXT NOT NULL,
  entity TEXT NOT NULL,
  expected_revision TEXT,
  manifest_object TEXT NOT NULL,
  operation TEXT NOT NULL CHECK (operation IN ('checkpoint', 'delete', 'restore', 'resolve', 'conflict')),
  device_id TEXT NOT NULL,
  recovery_generation TEXT NOT NULL,
  request_hash TEXT NOT NULL CHECK (length(request_hash) = 64),
  created_at INTEGER NOT NULL,
  superseded_at INTEGER,
  pinned INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
  PRIMARY KEY (vault_id, id),
  FOREIGN KEY (vault_id, manifest_object) REFERENCES sync_objects(vault_id, id),
  CHECK (operation = 'conflict' OR pinned = 0)
);
CREATE INDEX sync_revisions_history ON sync_revisions(vault_id, entity, created_at);
CREATE INDEX sync_revisions_expiry ON sync_revisions(pinned, superseded_at);

CREATE TABLE sync_heads (
  vault_id TEXT NOT NULL,
  entity TEXT NOT NULL,
  revision TEXT NOT NULL,
  PRIMARY KEY (vault_id, entity),
  FOREIGN KEY (vault_id, revision) REFERENCES sync_revisions(vault_id, id)
);

CREATE TABLE sync_membership (
  vault_id TEXT NOT NULL,
  revision TEXT NOT NULL,
  object TEXT NOT NULL,
  PRIMARY KEY (vault_id, revision, object),
  FOREIGN KEY (vault_id, revision) REFERENCES sync_revisions(vault_id, id) ON DELETE CASCADE,
  FOREIGN KEY (vault_id, object) REFERENCES sync_objects(vault_id, id)
);
CREATE INDEX sync_membership_objects ON sync_membership(vault_id, object);

CREATE TABLE sync_changes (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  vault_id TEXT NOT NULL,
  entity TEXT NOT NULL,
  revision TEXT NOT NULL,
  operation TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX sync_changes_cursor ON sync_changes(vault_id, sequence);

CREATE TRIGGER sync_membership_available BEFORE INSERT ON sync_membership BEGIN
  SELECT (CASE WHEN NOT EXISTS (
    SELECT 1 FROM sync_objects WHERE vault_id = NEW.vault_id AND id = NEW.object
      AND state IN ('uploaded', 'unreferenced')
  ) THEN RAISE(ABORT, 'object unavailable') END);
  SELECT (CASE WHEN EXISTS (
    SELECT 1 FROM sync_objects o JOIN sync_revisions r ON r.vault_id = o.vault_id
    WHERE o.vault_id = NEW.vault_id AND o.id = NEW.object AND r.id = NEW.revision
      AND o.kind = 'control_manifest' AND r.operation NOT IN ('delete', 'restore', 'resolve')
  ) THEN RAISE(ABORT, 'control capacity is reserved for recovery') END);
END;

CREATE TRIGGER sync_membership_reference AFTER INSERT ON sync_membership BEGIN
  UPDATE sync_objects SET refs = refs + 1, state = 'uploaded', charged = 1, unreferenced_at = NULL
  WHERE vault_id = NEW.vault_id AND id = NEW.object;
END;

CREATE TRIGGER sync_membership_release AFTER DELETE ON sync_membership BEGIN
  UPDATE sync_objects SET
    refs = refs - 1,
    state = (CASE WHEN refs = 1 THEN 'unreferenced' ELSE state END),
    charged = (CASE WHEN refs = 1 THEN 0 ELSE charged END),
    unreferenced_at = (CASE WHEN refs = 1 THEN unixepoch() * 1000 ELSE unreferenced_at END)
  WHERE vault_id = OLD.vault_id AND id = OLD.object;
END;

CREATE TRIGGER sync_revisions_cas BEFORE INSERT ON sync_revisions BEGIN
  SELECT (CASE WHEN NOT EXISTS (SELECT 1 FROM sync_accounts WHERE vault_id = NEW.vault_id AND active = 1)
    THEN RAISE(ABORT, 'inactive vault') END);
  SELECT (CASE WHEN NEW.recovery_generation IS NOT (
    SELECT recovery_generation FROM sync_accounts WHERE vault_id = NEW.vault_id
  ) THEN RAISE(ABORT, 'recovery generation changed') END);
  SELECT (CASE WHEN NEW.operation != 'conflict' AND
    (SELECT revision FROM sync_heads WHERE vault_id = NEW.vault_id AND entity = NEW.entity) IS NOT NEW.expected_revision
    THEN RAISE(ABORT, 'revision conflict') END);
  SELECT (CASE WHEN NEW.operation = 'conflict' AND NEW.pinned != 1
    THEN RAISE(ABORT, 'conflict must remain pinned') END);
  SELECT (CASE WHEN NOT EXISTS (
    SELECT 1 FROM sync_objects WHERE vault_id = NEW.vault_id AND id = NEW.manifest_object
      AND kind IN ('manifest', 'control_manifest') AND state IN ('uploaded', 'unreferenced')
  ) THEN RAISE(ABORT, 'manifest unavailable') END);
END;

CREATE TRIGGER sync_revisions_immutable BEFORE UPDATE ON sync_revisions
WHEN NEW.id != OLD.id OR NEW.vault_id != OLD.vault_id OR NEW.entity != OLD.entity
  OR NEW.expected_revision IS NOT OLD.expected_revision OR NEW.manifest_object != OLD.manifest_object
  OR NEW.operation != OLD.operation OR NEW.device_id != OLD.device_id OR NEW.created_at != OLD.created_at
  OR NEW.recovery_generation != OLD.recovery_generation OR NEW.request_hash != OLD.request_hash
BEGIN SELECT RAISE(ABORT, 'immutable revision'); END;

CREATE TRIGGER sync_revisions_publish AFTER INSERT ON sync_revisions BEGIN
  INSERT INTO sync_membership (vault_id, revision, object) VALUES (NEW.vault_id, NEW.id, NEW.manifest_object);
  UPDATE sync_revisions SET superseded_at = NEW.created_at WHERE vault_id = NEW.vault_id
    AND id = (SELECT revision FROM sync_heads WHERE vault_id = NEW.vault_id AND entity = NEW.entity)
    AND NEW.operation != 'conflict';
  INSERT INTO sync_heads (vault_id, entity, revision)
    SELECT NEW.vault_id, NEW.entity, NEW.id WHERE NEW.operation != 'conflict'
    ON CONFLICT (vault_id, entity) DO UPDATE SET revision = excluded.revision;
  INSERT INTO sync_changes (vault_id, entity, revision, operation, created_at)
    VALUES (NEW.vault_id, NEW.entity, NEW.id, NEW.operation, NEW.created_at);
END;

CREATE TRIGGER sync_revisions_preserve BEFORE DELETE ON sync_revisions BEGIN
  SELECT (CASE WHEN OLD.pinned = 1 OR EXISTS (
    SELECT 1 FROM sync_heads WHERE vault_id = OLD.vault_id AND revision = OLD.id
  ) THEN RAISE(ABORT, 'protected revision') END);
END;

CREATE TABLE sync_jobs (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  payload TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  available_at INTEGER NOT NULL,
  lease_token TEXT,
  lease_until INTEGER,
  completed_at INTEGER,
  failed_at INTEGER,
  failure_code TEXT
);
CREATE INDEX sync_jobs_ready ON sync_jobs(available_at, lease_until) WHERE completed_at IS NULL AND failed_at IS NULL;
