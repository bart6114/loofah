import { beforeEach, expect, test } from "vitest";

import { isFinishedAudioImportStatus, useAudioImport } from "./audio-import";

beforeEach(() =>
  useAudioImport.setState({
    items: [],
    activeItemId: null,
    completionAnnounced: false,
    dialogOpen: false,
  }),
);

test("partial imports retain the session and let the queue continue without reimporting missing audio", () => {
  const store = useAudioImport.getState();
  store.enqueue([
    { kind: "path", path: "/tmp/one.wav", name: "one.wav" },
    { kind: "path", path: "/tmp/two.wav", name: "two.wav" },
  ]);
  store.claimNext();
  const firstId = useAudioImport.getState().activeItemId!;
  store.setItemSession(firstId, "saved-session");
  store.finishItem(firstId, "Incomplete transcript", "incomplete");
  const partial = useAudioImport.getState().items[0];
  expect(partial.status).toBe("incomplete");
  expect(partial.sessionId).toBe("saved-session");
  expect(isFinishedAudioImportStatus(partial.status)).toBe(true);
  store.retryItem(firstId);
  expect(useAudioImport.getState().items[0].status).toBe("incomplete");
  store.claimNext();
  expect(useAudioImport.getState().activeItemId).not.toBe(firstId);
});
