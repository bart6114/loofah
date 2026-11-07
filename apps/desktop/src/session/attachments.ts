import { enqueueSessionAudioOperation } from "./audio-operations";

import { enqueueDatabaseWrite } from "~/shared/write-queue";
import { commands } from "~/types/tauri.gen";

// Settles a just-finished recording at the canonical `<session dir>/audio.<ext>` via the
// session store, and returns where it ended up — the store may relocate it, so callers
// holding the capture backend's path must re-point at this one.
export async function catalogLocalSessionAudio(
  sessionId: string,
  sourcePath: string,
): Promise<string> {
  const result = await commands.sessionStoreAudio(sessionId, sourcePath);
  if (result.status === "error") {
    throw new Error(result.error);
  }
  return result.data;
}

export async function deleteSessionAudio(
  inputSessionId: string,
  canDelete: () => boolean,
  remove: () => Promise<void>,
): Promise<boolean> {
  const sessionId = requireText(inputSessionId, "session ID", 512);
  return enqueueSessionAudioOperation(sessionId, () =>
    enqueueDatabaseWrite(`session:${sessionId}`, async () => {
      if (!canDelete()) {
        return false;
      }
      await remove();
      return true;
    }),
  );
}

function requireText(
  value: unknown,
  label: string,
  maxLength: number,
  allowEmpty = false,
) {
  if (
    typeof value !== "string" ||
    (!allowEmpty && value.length === 0) ||
    value.length > maxLength ||
    value.trim() !== value ||
    /[\u0000-\u001f\u007f]/.test(value)
  ) {
    throw new Error(`invalid ${label}`);
  }
  return value;
}
