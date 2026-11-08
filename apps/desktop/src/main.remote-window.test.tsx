import { act, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
const entry = vi.hoisted(() => ({
  bootstrap: vi.fn(async () => ({
    info: {
      startup: { selection: { kind: "remote" } },
      connection: { id: "saved-profile" },
    },
    error: null,
  })),
  bootstrapRoot: vi.fn(),
  startup: vi.fn(),
  config: vi.fn(),
  settings: vi.fn(),
  theme: vi.fn(),
  bridge: vi.fn(),
  artifacts: vi.fn(),
  device: vi.fn(),
  floating: vi.fn(),
  roots: [] as Array<{ unmount: () => void }>,
}));
vi.mock("react-dom/client", async (original) => {
  const actual = await original<typeof import("react-dom/client")>();
  const createRoot: typeof actual.createRoot = (...args) => {
    const root = actual.createRoot(...args);
    entry.roots.push(root);
    return root;
  };
  return { ...actual, default: { ...actual.default, createRoot }, createRoot };
});
vi.mock("@tanstack/react-router", () => ({
  createRouter: () => ({}),
  RouterProvider: () => <output>original scoped note route</output>,
}));
vi.mock("~/routeTree.gen", () => ({ routeTree: {} }));
vi.mock("@hypr/plugin-windows", () => ({
  getCurrentWebviewWindowLabel: () =>
    "remote-note-4b79222d-0873-4ce0-a210-e08c838c1ca4",
  init: vi.fn(),
}));
vi.mock("@hypr/ui/components/ui/toast", () => ({ Toaster: () => null }));
vi.mock("~/shared/application/desktop-bootstrap", () => ({
  loadDesktopBootstrap: entry.bootstrap,
  DesktopBootstrapRoot: entry.bootstrapRoot,
}));
vi.mock("~/shared/startup-boundary", () => ({
  StartupBoundary: entry.startup,
}));
vi.mock("~/shared/config/store", () => ({ initConfigStore: entry.config }));
vi.mock("~/settings/queries", () => ({
  initializeApplicationSettings: entry.settings,
}));
vi.mock("~/shared/theme/apply", () => ({
  bootstrapThemeFromSettings: entry.theme,
  normalizeThemePreference: () => "light",
}));
vi.mock("~/shared/theme/catalog", () => ({
  getDesignTheme: () => ({}),
  resolveThemeAppearance: () => "light",
}));
vi.mock("~/shared/theme/provider", () => ({
  AppThemeProvider: ({ children }: any) => children,
}));
vi.mock("~/i18n/provider", () => ({
  AppI18nProvider: ({ children }: any) => children,
}));
vi.mock("~/shared/config", () => ({ useConfigValue: () => null }));
vi.mock("~/shared/control", () => ({
  ErrorComponent: () => null,
  NotFoundComponent: () => null,
}));
vi.mock("~/shared/performance", () => ({
  startPerformanceDiagnostics: vi.fn(),
}));
vi.mock("~/shared/app-exit", () => ({ initializeAppExitFlush: vi.fn() }));
vi.mock("~/ai/task-window-sync", () => ({
  AITaskWindowSyncBridge: entry.bridge,
}));
vi.mock("~/services/artifact-invalidation", () => ({
  ArtifactInvalidationSync: entry.artifacts,
}));
vi.mock("~/services/event-listeners", () => ({
  DeviceEventListeners: entry.device,
}));
vi.mock("~/meeting-float/host", () => ({
  FloatingMeetingWindowHost: entry.floating,
}));
vi.mock(
  "~/session/components/note-input/transcript/regenerate-confirm",
  () => ({ RegenerateTranscriptConfirmDialog: () => null }),
);
vi.mock("~/store/zustand/ai-task", () => ({ createAITaskStore: () => ({}) }));
vi.mock("~/store/zustand/listener/instance", () => ({ listenerStore: {} }));
it("real child entry bypasses saved Remote main shell and all Local startup services", async () => {
  const root = document.createElement("div");
  root.id = "root";
  document.body.append(root);
  await act(async () => {
    await import("./main");
  });
  await waitFor(() =>
    expect(screen.getByText("original scoped note route")).toBeTruthy(),
  );
  for (const call of [
    entry.bootstrap,
    entry.bootstrapRoot,
    entry.startup,
    entry.config,
    entry.settings,
    entry.theme,
    entry.bridge,
    entry.artifacts,
    entry.device,
    entry.floating,
  ])
    expect(call).not.toHaveBeenCalled();
  await act(async () => {
    for (const mounted of entry.roots.splice(0)) mounted.unmount();
  });
  root.remove();
});
