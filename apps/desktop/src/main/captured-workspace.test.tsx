import {
  QueryClient,
  QueryClientProvider,
  onlineManager,
} from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  calls: [] as string[],
  secret: vi.fn(async () => ({ status: "ok", data: null })),
  local: vi.fn(),
  settings: vi.fn(async () => ({
    status: "ok",
    data: {
      current_llm_provider: "openai",
      current_llm_model: "gpt-test",
      ai_providers: {
        "llm:openai": {
          type: "llm",
          base_url: "https://local-provider.invalid",
        },
      },
    },
  })),
}));
vi.mock("~/types/tauri.gen", async (original) => {
  const actual = await original<typeof import("~/types/tauri.gen")>();
  const allowed: Record<string, () => unknown> = {
    showDevtool: () => false,
    draftCacheGet: () => ({ status: "ok", data: null }),
    draftCacheRecovery: () => ({ status: "ok", data: null }),
    draftCacheInventory: () => ({ status: "ok", data: [] }),
    draftCachePut: () => ({ status: "ok", data: null }),
    draftCacheRemove: () => ({ status: "ok", data: null }),
  };
  return {
    ...actual,
    commands: new Proxy(actual.commands, {
      get: (_target, name) => async () => {
        native.calls.push(String(name));
        const call = allowed[String(name)];
        if (!call)
          throw new Error(`Unexpected Native command: ${String(name)}`);
        return call();
      },
    }),
  };
});
vi.mock("@hypr/plugin-store2", () => ({
  commands: { getSecret: native.secret },
}));
vi.mock("@hypr/plugin-settings", () => ({
  commands: {
    getConfig: native.settings,
    setConfigValues: vi.fn(async () => ({ status: "ok", data: null })),
  },
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(),
}));
vi.mock("@tanstack/react-virtual", async (original) => {
  const actual = await original<typeof import("@tanstack/react-virtual")>();
  return {
    ...actual,
    useVirtualizer: (options: Parameters<typeof actual.useVirtualizer>[0]) =>
      actual.useVirtualizer({
        ...options,
        initialRect: { width: 240, height: 500 },
        observeElementRect: (_instance, callback) => {
          callback({ width: 240, height: 500 });
          return () => {};
        },
      }),
  };
});
vi.mock("@tauri-apps/plugin-os", () => ({ platform: () => "macos" }));
vi.mock("./update-banner", () => ({
  useDesktopUpdateControl: () => ({ status: null, version: null }),
  SidebarTimelineUpdateButton: () => null,
}));
vi.mock("~/services/enhancer", () => ({
  initEnhancerService: native.local,
  getEnhancerService: native.local,
}));
vi.mock("~/stt/audio-import-worker", () => ({
  AudioImportWorker: () => {
    native.local();
    return null;
  },
}));
vi.mock("~/services/event-listeners", () => ({
  EventListeners: () => {
    native.local();
    return null;
  },
}));
vi.mock("~/contexts/notifications", () => ({
  NotificationProvider: ({ children }: { children: React.ReactNode }) => {
    native.local();
    return children;
  },
  useNotifications: () => ({
    shouldShowBadge: false,
    hasActiveDownload: false,
    notificationCount: 0,
  }),
}));
vi.mock("@hypr/plugin-local-stt", async (original) => {
  const actual = await original<typeof import("@hypr/plugin-local-stt")>();
  return {
    ...actual,
    commands: new Proxy(actual.commands, {
      get: (_target, name) => async () => {
        native.calls.push(`model:${String(name)}`);
        throw new Error("Unexpected Local model request");
      },
    }),
  };
});

import { scopedAttachmentPresentation } from "@hypr/editor/attachment-presentation";

import { CapturedMainWorkspace, MainWorkspace } from "./captured-workspace";

import type {
  NoteEditorSnapshot,
  MetadataSnapshot,
  SessionSummaryQuery,
} from "~/shared/application/protocol.gen";
import type {
  VaultClient,
  VaultClientEvent,
} from "~/shared/application/vault-client";
import { VaultScope } from "~/shared/application/vault-scope";
import { resetConfigStoreForTests } from "~/shared/config/store";
import { useTabs, useTabsStore } from "~/store/zustand/tabs";
import { deviceFixture } from "~/store/zustand/tabs/scoped-test-utils";

const scopes: VaultScope[] = [];
const caches: QueryClient[] = [];
beforeEach(() => {
  vi.clearAllMocks();
  native.calls.length = 0;
  resetConfigStoreForTests();
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  HTMLElement.prototype.scrollIntoView = vi.fn();
  HTMLElement.prototype.scrollTo = vi.fn();
});
afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
  for (const cache of caches.splice(0)) cache.clear();
  onlineManager.setOnline(true);
  vi.unstubAllGlobals();
});
function fixture(
  id = "remote:test",
  networkMode: "always" | "online" = "online",
) {
  let receive!: (event: VaultClientEvent) => void;
  const metadata = (sessionId: string): MetadataSnapshot => ({
    revision: "m1",
    meta: {
      id: sessionId,
      title: `${id} ${sessionId}`,
      created_at: "2026-11-08T10:00:00Z",
      started_at: null,
      ended_at: null,
      tags: [],
    },
  });
  const note = (sessionId: string): NoteEditorSnapshot => ({
    note: {
      revision: "n1",
      title: `${id} ${sessionId}`,
      markdown: `Private body ${id} ${sessionId}`,
    },
    tasks: { revision: "t1", tasks: [] },
  });
  const summaries = ["one", "two"].map((sessionId) => ({
    id: sessionId,
    title: `${id} ${sessionId}`,
    created_at: "2026-11-08T10:00:00Z",
  }));
  const client = {
    scope: id,
    networkMode,
    canEdit: true,
    attachmentPresentation: scopedAttachmentPresentation,
    features: [
      "content_read",
      "timeline_read",
      "session_summaries",
      "search",
      "note_editor_read",
      "note_editor_write",
      "metadata_read",
      "metadata_write",
      "transcript_overview",
      "document_editor_read",
      "session_cleanup",
    ],
    subscribe: vi.fn(async (callback: (event: VaultClientEvent) => void) => {
      receive = callback;
      callback({ kind: "connection", state: "connected", error: null });
      return () => {};
    }),
    readMetadata: vi.fn(async (sessionId: string) => metadata(sessionId)),
    readNoteEditor: vi.fn(async (sessionId: string) => note(sessionId)),
    documents: vi.fn(async () => []),
    transcriptOverview: vi.fn(async () => ({
      has_words: false,
      speaker_labels: [],
      started_at: null,
      ended_at: null,
    })),
    transcripts: vi.fn(async () => []),
    people: vi.fn(async () => []),
    attachments: vi.fn(async () => ({ files: [], next_cursor: null })),
    audioMetadata: vi.fn(async () => null),
    openRecording: vi.fn(async () => null),
    sessionSummaries: vi.fn(async (input: SessionSummaryQuery) =>
      input.kind === "ids"
        ? summaries.filter((item) => input.ids.includes(item.id))
        : summaries,
    ),
    search: vi.fn(async () => ({
      count: 1,
      hits: [
        {
          score: 1,
          document: {
            id: "two",
            doc_type: "session",
            title: `${id} two`,
            content: "Search content",
            created_at: "2026-11-08T10:00:00Z",
          },
          title_snippet: null,
          content_snippet: null,
        },
      ],
    })),
    timelineHeaders: vi.fn(
      async (
        input: import("~/shared/application/protocol.gen").TimelineQuery,
      ) => ({
        revision: "r1",
        items:
          input.view.kind === "date"
            ? summaries.map((header) => ({
                header: {
                  ...header,
                  folder: null,
                  tags: [],
                  author: null,
                  has_transcript_words: false,
                },
                timestamp_ms: Date.parse(header.created_at),
              }))
            : [],
        total_count: input.view.kind === "date" ? 2 : 0,
        future_count: 0,
        undated_count: 0,
        next_cursor: null,
      }),
    ),
    deleteEmptySession: vi.fn(async () => ({ kind: "deleted" as const })),
    timelineFacets: vi.fn(async () => ({
      revision: "r1",
      nodes: [],
      untagged_count: 2,
    })),
  };
  return { client, emit: (event: VaultClientEvent) => receive(event) };
}
function NavigationProbe() {
  const open = useTabs((state) => state.openCurrent);
  const store = useTabsStore();
  return (
    <>
      <button
        onClick={() =>
          open({ type: "settings", state: { tab: "intelligence" } })
        }
      >
        Unsafe summary settings
      </button>
      <button
        onClick={() =>
          open({
            type: "sessions",
            id: "one",
            state: { view: { type: "raw" }, autoStart: null },
          })
        }
      >
        Open one
      </button>
      <button
        onClick={() => {
          const tab = store.getState().currentTab;
          if (tab) store.getState().close(tab);
        }}
      >
        Close current
      </button>
    </>
  );
}
function cache() {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  caches.push(qc);
  return qc;
}
async function mount(id = "remote:test", mode: "always" | "online" = "online") {
  const f = fixture(id, mode);
  const scope = new VaultScope(f.client as unknown as VaultClient);
  scopes.push(scope);
  await scope.start();
  const device = deviceFixture();
  const ui = render(
    <QueryClientProvider client={cache()}>
      <CapturedMainWorkspace
        scope={scope}
        deviceState={device.device}
        auxiliary={<NavigationProbe />}
      />
    </QueryClientProvider>,
  );
  return { ...f, ...ui, scope, device };
}
it("mounts the actual main shell, timeline and compound note editor using only the captured client", async () => {
  const view = await mount();
  await screen.findByTestId("main-app-shell");
  await waitFor(() => expect(view.client.timelineHeaders).toHaveBeenCalled());
  fireEvent.click(screen.getByRole("button", { name: "Open one" }));
  await screen.findByText("Private body remote:test one");
  expect(view.client.readNoteEditor).toHaveBeenCalled();
  expect(view.client.timelineHeaders).toHaveBeenCalled();
  expect(native.secret).not.toHaveBeenCalled();
  expect(native.local).not.toHaveBeenCalled();
  expect(
    native.calls.filter(
      (name) => !name.startsWith("draftCache") && name !== "showDevtool",
    ),
  ).toEqual([]);
});
it("navigates search and guarded settings in the real shell without mounting Local models or credentials", async () => {
  const view = await mount();
  await screen.findByRole("button", { name: "Open one" });
  fireEvent.click(screen.getByRole("button", { name: "Open one" }));
  await screen.findByText("Private body remote:test one");
  fireEvent.click(screen.getByText("Search notes…"));
  const input = await screen.findByRole("combobox");
  fireEvent.change(input, { target: { value: "two" } });
  await waitFor(() => expect(view.client.search).toHaveBeenCalled());
  fireEvent.keyDown(input, { key: "Escape" });
  fireEvent.click(
    screen.getByRole("button", { name: "Unsafe summary settings" }),
  );
  await screen.findByText("This operation is unavailable for this connection.");
  expect(native.secret).not.toHaveBeenCalled();
  expect(native.local).not.toHaveBeenCalled();
  expect(
    native.calls.some(
      (name) => name.startsWith("model:") || name.startsWith("session"),
    ),
  ).toBe(false);
});
it("keeps Local always/offline/login-free and does not authorize cleanup without capture inventory", async () => {
  onlineManager.setOnline(false);
  const view = await mount("local:test", "always");
  fireEvent.click(await screen.findByRole("button", { name: "Open one" }));
  await screen.findByText("Private body local:test one");
  expect(screen.queryByText(/Sign in|Log in/i)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Close current" }));
  expect(native.local).not.toHaveBeenCalled();
  expect(view.client.deleteEmptySession).not.toHaveBeenCalled();
});
it("accepts an explicit client without connecting any implicit Local provider", async () => {
  const f = fixture();
  const device = deviceFixture();
  render(
    <StrictMode>
      <QueryClientProvider client={cache()}>
        <MainWorkspace
          client={f.client as unknown as VaultClient}
          deviceState={device.device}
        />
      </QueryClientProvider>
    </StrictMode>,
  );
  await screen.findByTestId("main-app-shell");
  expect(f.client.subscribe).toHaveBeenCalled();
  expect(
    native.calls.some((name) =>
      /vaultInfo|applicationVaultInfo|sessionGet/.test(name),
    ),
  ).toBe(false);
  expect(native.local).not.toHaveBeenCalled();
});

it("retires the entire main subtree for a replacement Scope with the same scope string and note IDs", async () => {
  const a = fixture("same"),
    b = fixture("same");
  const first = new VaultScope(a.client as unknown as VaultClient),
    second = new VaultScope(b.client as unknown as VaultClient);
  scopes.push(first, second);
  await first.start();
  await second.start();
  let finish!: (value: NoteEditorSnapshot) => void;
  b.client.readMetadata.mockResolvedValue({
    revision: "m2",
    meta: {
      id: "one",
      title: "Replacement title",
      created_at: "2026-11-08T10:00:00Z",
      started_at: null,
      ended_at: null,
      tags: [],
    },
  });
  b.client.readNoteEditor.mockReturnValue(
    new Promise<NoteEditorSnapshot>((resolve) => {
      finish = resolve;
    }),
  );
  const device = deviceFixture(),
    qc = cache();
  const element = (scope: VaultScope) => (
    <QueryClientProvider client={qc}>
      <CapturedMainWorkspace
        scope={scope}
        deviceState={device.device}
        auxiliary={<NavigationProbe />}
      />
    </QueryClientProvider>
  );
  const ui = render(element(first));
  fireEvent.click(await screen.findByRole("button", { name: "Open one" }));
  await screen.findByText("Private body same one");
  ui.rerender(element(second));
  expect(screen.queryByText("Private body same one")).toBeNull();
  fireEvent.click(await screen.findByRole("button", { name: "Open one" }));
  await act(async () => {
    await new Promise((done) => setTimeout(done, 150));
    finish({
      note: {
        revision: "n2",
        title: "Replacement title",
        markdown: "Replacement body",
      },
      tasks: { revision: "t2", tasks: [] },
    });
  });
  await screen.findByText("Replacement body");
  expect(screen.queryByText("Private body same one")).toBeNull();
  expect(native.secret).not.toHaveBeenCalled();
});

it("opens existing device App settings but hides the whole private workspace on terminal access loss", async () => {
  const view = await mount();
  await screen.findByTestId("main-app-shell");
  const button = view.container.querySelector<HTMLButtonElement>(
    'button[aria-label="Settings"]',
  );
  expect(button).not.toBeNull();
  fireEvent.click(button!);
  await screen.findByText("Timezone");
  expect(native.secret).not.toHaveBeenCalled();
  expect(native.local).not.toHaveBeenCalled();
  act(() =>
    view.emit({ kind: "connection", state: "access_denied", error: null }),
  );
  expect(screen.queryByTestId("main-app-shell")).toBeNull();
  expect(screen.getByRole("alert").textContent).toBe("Vault unavailable");
  act(() => view.emit({ kind: "connection", state: "connected", error: null }));
  expect(screen.queryByTestId("main-app-shell")).toBeNull();
});
