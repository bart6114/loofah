CREATE TRIGGER sync_device_revoke_sessions AFTER UPDATE OF revoked_at ON sync_devices
WHEN OLD.revoked_at IS NULL AND NEW.revoked_at IS NOT NULL BEGIN
  DELETE FROM session WHERE id IN (
    SELECT session_id FROM sync_device_sessions WHERE vault_id = NEW.vault_id AND device_id = NEW.id
  );
  DELETE FROM sync_enrollment_challenges WHERE vault_id = NEW.vault_id AND device_id = NEW.id;
END;
