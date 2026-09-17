ALTER TABLE sync_enrollment_challenges ADD COLUMN pairing_required INTEGER NOT NULL DEFAULT 0 CHECK (pairing_required IN (0, 1));
ALTER TABLE sync_enrollment_challenges ADD COLUMN approver_session TEXT;

CREATE TRIGGER sync_pairing_receipt BEFORE INSERT ON sync_enrollment_receipts
WHEN EXISTS (SELECT 1 FROM sync_enrollment_challenges WHERE id = NEW.challenge AND pairing_required = 1)
BEGIN
  SELECT (CASE WHEN NOT EXISTS (
    SELECT 1 FROM sync_enrollment_challenges c
    JOIN sync_device_sessions b ON b.session_id = c.approver_session AND b.vault_id = c.vault_id
    JOIN sync_devices d ON d.vault_id = b.vault_id AND d.id = b.device_id
    JOIN session s ON s.id = b.session_id AND s.userId = c.user_id
    WHERE c.id = NEW.challenge AND c.approver_session != c.session_id
      AND d.revoked_at IS NULL AND s.expiresAt > NEW.created_at
  ) THEN RAISE(ABORT, 'pairing approval unavailable') END);
END;

CREATE TRIGGER sync_pairing_revoke AFTER UPDATE OF revoked_at ON sync_devices
WHEN NEW.revoked_at IS NOT NULL AND OLD.revoked_at IS NULL
BEGIN
  DELETE FROM sync_enrollment_challenges WHERE approver_session IN (
    SELECT session_id FROM sync_device_sessions WHERE vault_id = NEW.vault_id AND device_id = NEW.id
  );
END;

CREATE TRIGGER sync_pairing_session_delete BEFORE DELETE ON session
BEGIN
  DELETE FROM sync_enrollment_challenges WHERE approver_session = OLD.id;
END;
