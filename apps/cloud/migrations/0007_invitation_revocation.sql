ALTER TABLE sync_invitations ADD COLUMN revoked_at INTEGER;
CREATE INDEX sync_invitations_email ON sync_invitations(email);

DROP TRIGGER sync_user_invitation;
CREATE TRIGGER sync_user_invitation BEFORE INSERT ON user BEGIN
  SELECT (CASE WHEN NOT EXISTS (SELECT 1 FROM sync_invitations WHERE id = NEW.invitationHash
    AND email = lower(trim(NEW.email)) AND consumed_user IS NULL AND revoked_at IS NULL
    AND expires_at > unixepoch() * 1000)
    THEN RAISE(ABORT, 'invitation unavailable') END);
  SELECT (CASE WHEN (SELECT count(*) FROM user) >= (SELECT account_limit FROM sync_beta_settings WHERE id = 1)
    THEN RAISE(ABORT, 'beta capacity reached') END);
END;

CREATE TRIGGER sync_invitation_reissue BEFORE INSERT ON sync_invitations BEGIN
  UPDATE sync_invitations SET revoked_at = unixepoch() * 1000
    WHERE email = NEW.email AND consumed_user IS NULL AND revoked_at IS NULL;
END;
