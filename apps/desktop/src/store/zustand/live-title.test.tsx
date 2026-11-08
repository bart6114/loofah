import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, expect, it } from "vitest";

import {
  hasLiveSessionTitleDraft,
  liveSessionTitleDraft,
  useLiveTitle,
  useSessionTitle,
  type LiveTitleDraft,
} from "./live-title";

import { createSessionSourceHost } from "~/session/session-source.test-support";

const hosts: ReturnType<typeof createSessionSourceHost>[] = [];
function fixture(scope: string = crypto.randomUUID(), legacy = false) {
  const host = createSessionSourceHost("always", {
    scope,
    features: legacy ? ["legacy_desktop_enhance"] : [],
  });
  hosts.push(host);
  const draft: LiveTitleDraft = {
    scope: host.scope,
    sessionId: "same",
    owner: {},
    title: "Private typing",
    revision: "m0",
    baselineTitle: "",
    uncertain: false,
    legacy,
  };
  useLiveTitle.getState().setTitle(draft);
  return { host, draft };
}
beforeEach(() => useLiveTitle.setState({ titles: {} }));
afterEach(() => {
  cleanup();
  hosts.splice(0).forEach((host) => host.dispose());
});
it("keeps identical session IDs in separate scopes and clears only the exact draft version", () => {
  const first = fixture(),
    other = fixture();
  const newer = { ...first.draft, title: "Newer typing" };
  useLiveTitle.getState().replaceTitle(first.draft, newer);
  useLiveTitle.getState().clearTitle(first.draft);
  expect(liveSessionTitleDraft(first.host.scope, "same")).toBe(newer);
  expect(liveSessionTitleDraft(other.host.scope, "same")).toBe(other.draft);
  useLiveTitle.getState().clearTitle(newer);
  expect(liveSessionTitleDraft(first.host.scope, "same")).toBeUndefined();
  expect(liveSessionTitleDraft(other.host.scope, "same")).toBe(other.draft);
});
it("does not admit an old scope clear/update into a same-string replacement", () => {
  const old = fixture("same-scope"),
    replacement = fixture("same-scope");
  useLiveTitle
    .getState()
    .replaceTitle(old.draft, { ...old.draft, uncertain: true });
  useLiveTitle.getState().clearTitle(old.draft);
  expect(liveSessionTitleDraft(old.host.scope, "same")?.title).toBe(
    "Private typing",
  );
  expect(liveSessionTitleDraft(old.host.scope, "same")?.uncertain).toBe(true);
  expect(liveSessionTitleDraft(replacement.host.scope, "same")).toBe(
    replacement.draft,
  );
});
it("exposes legacy AI protection only for explicitly admitted local drafts, including empty titles", () => {
  fixture("remote");
  expect(hasLiveSessionTitleDraft("same")).toBe(false);
  const local = fixture("local", true);
  const empty = { ...local.draft, title: "" };
  useLiveTitle.getState().replaceTitle(local.draft, empty);
  expect(hasLiveSessionTitleDraft("same")).toBe(true);
  useLiveTitle.getState().clearTitle(empty);
  expect(hasLiveSessionTitleDraft("same")).toBe(false);
});
it("shows only the captured scope preview and hides it permanently on terminal access loss", () => {
  const value = fixture();
  const view = renderHook(() => useSessionTitle("same", "Saved"), {
    wrapper: value.host.Host,
  });
  expect(view.result.current).toBe("Private typing");
  act(() =>
    value.host.emit({ kind: "connection", state: "reconnecting", error: null }),
  );
  expect(view.result.current).toBe("Private typing");
  act(() =>
    value.host.emit({
      kind: "connection",
      state: "access_denied",
      error: null,
    }),
  );
  expect(view.result.current).toBe("Untitled");
  act(() =>
    value.host.emit({ kind: "connection", state: "connected", error: null }),
  );
  expect(view.result.current).toBe("Untitled");
});
