ALTER TABLE sync_accounts ADD COLUMN enrollment_authority TEXT CHECK (enrollment_authority IS NULL OR length(enrollment_authority) = 64);
CREATE TRIGGER sync_authority_immutable BEFORE UPDATE OF enrollment_authority ON sync_accounts
WHEN OLD.enrollment_authority IS NOT NULL AND NEW.enrollment_authority IS NOT OLD.enrollment_authority
BEGIN SELECT RAISE(ABORT, 'enrollment authority cannot be replaced'); END;

CREATE TABLE sync_devices (
  vault_id TEXT NOT NULL REFERENCES sync_accounts(vault_id),
  id TEXT NOT NULL,
  public_key TEXT NOT NULL CHECK (length(public_key) = 64),
  enrolled_at INTEGER NOT NULL,
  revoked_at INTEGER,
  PRIMARY KEY (vault_id, id)
);

CREATE TABLE sync_device_sessions (
  session_id TEXT PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE,
  vault_id TEXT NOT NULL,
  device_id TEXT NOT NULL,
  FOREIGN KEY (vault_id, device_id) REFERENCES sync_devices(vault_id, id)
);

CREATE TABLE sync_enrollment_challenges (
  id TEXT PRIMARY KEY,
  vault_id TEXT NOT NULL REFERENCES sync_accounts(vault_id),
  session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  user_id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('enroll', 'bind')),
  device_id TEXT NOT NULL,
  public_key TEXT NOT NULL,
  authority TEXT NOT NULL,
  generation TEXT NOT NULL,
  nonce TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  consumed_at INTEGER
);
CREATE INDEX sync_enrollment_expiry ON sync_enrollment_challenges(expires_at);
CREATE TRIGGER sync_enrollment_limit BEFORE INSERT ON sync_enrollment_challenges BEGIN
  SELECT (CASE WHEN (SELECT count(*) FROM sync_enrollment_challenges WHERE user_id = NEW.user_id
    AND consumed_at IS NULL AND expires_at > unixepoch() * 1000) >= 20
    THEN RAISE(ABORT, 'too many pending enrollments') END);
END;

CREATE TABLE sync_enrollment_receipts (
  challenge TEXT PRIMARY KEY REFERENCES sync_enrollment_challenges(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL
);

CREATE TRIGGER sync_enrollment_accept BEFORE INSERT ON sync_enrollment_receipts BEGIN
  SELECT (CASE WHEN NOT EXISTS (
    SELECT 1 FROM sync_enrollment_challenges c JOIN sync_accounts a ON a.vault_id = c.vault_id
      JOIN session s ON s.id = c.session_id JOIN user u ON u.id = s.userId
    WHERE c.id = NEW.challenge AND c.consumed_at IS NULL AND c.expires_at > NEW.created_at
      AND s.userId = c.user_id AND a.user_id = c.user_id AND u.emailVerified = 1 AND a.active = 1
      AND a.recovery_generation = c.generation AND (a.enrollment_authority IS NULL OR a.enrollment_authority = c.authority)
      AND (c.kind = 'enroll' OR EXISTS (SELECT 1 FROM sync_devices d WHERE d.vault_id = c.vault_id
        AND d.id = c.device_id AND d.public_key = c.public_key AND d.revoked_at IS NULL))
  ) THEN RAISE(ABORT, 'enrollment unavailable') END);
END;

CREATE TRIGGER sync_enrollment_publish AFTER INSERT ON sync_enrollment_receipts BEGIN
  UPDATE sync_accounts SET enrollment_authority = (SELECT authority FROM sync_enrollment_challenges WHERE id = NEW.challenge)
    WHERE vault_id = (SELECT vault_id FROM sync_enrollment_challenges WHERE id = NEW.challenge) AND enrollment_authority IS NULL;
  INSERT INTO sync_devices (vault_id, id, public_key, enrolled_at)
    SELECT vault_id, device_id, public_key, NEW.created_at FROM sync_enrollment_challenges WHERE id = NEW.challenge AND kind = 'enroll';
  INSERT INTO sync_device_sessions (session_id, vault_id, device_id)
    SELECT session_id, vault_id, device_id FROM sync_enrollment_challenges WHERE id = NEW.challenge
    ON CONFLICT (session_id) DO UPDATE SET vault_id = excluded.vault_id, device_id = excluded.device_id;
  UPDATE sync_enrollment_challenges SET consumed_at = NEW.created_at WHERE id = NEW.challenge;
END;

CREATE TRIGGER sync_device_identity_immutable BEFORE UPDATE ON sync_devices
WHEN NEW.vault_id != OLD.vault_id OR NEW.id != OLD.id OR NEW.public_key != OLD.public_key
  OR NEW.enrolled_at != OLD.enrolled_at OR (OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS NOT OLD.revoked_at)
BEGIN SELECT RAISE(ABORT, 'immutable device identity'); END;
