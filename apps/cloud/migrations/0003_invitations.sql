CREATE TABLE sync_beta_settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  account_limit INTEGER NOT NULL DEFAULT 25 CHECK (account_limit > 0),
  writes_enabled INTEGER NOT NULL DEFAULT 1 CHECK (writes_enabled IN (0, 1))
);
INSERT INTO sync_beta_settings (id) VALUES (1);

CREATE TABLE sync_invitations (
  id TEXT PRIMARY KEY CHECK (length(id) = 64),
  email TEXT NOT NULL CHECK (email = lower(trim(email))),
  expires_at INTEGER NOT NULL,
  consumed_user TEXT UNIQUE,
  consumed_at INTEGER
);

CREATE TRIGGER sync_user_invitation BEFORE INSERT ON user BEGIN
  SELECT (CASE WHEN NOT EXISTS (SELECT 1 FROM sync_invitations WHERE id = NEW.invitationHash
    AND email = lower(trim(NEW.email)) AND consumed_user IS NULL AND expires_at > unixepoch() * 1000)
    THEN RAISE(ABORT, 'invitation unavailable') END);
  SELECT (CASE WHEN (SELECT count(*) FROM user) >= (SELECT account_limit FROM sync_beta_settings WHERE id = 1)
    THEN RAISE(ABORT, 'beta capacity reached') END);
END;

CREATE TRIGGER sync_user_consume_invitation AFTER INSERT ON user BEGIN
  UPDATE sync_invitations SET consumed_user = NEW.id, consumed_at = unixepoch() * 1000 WHERE id = NEW.invitationHash;
END;

CREATE TABLE sync_request_limits (
  key TEXT PRIMARY KEY,
  count INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE INDEX sync_request_limits_expiry ON sync_request_limits(expires_at);
