import "@fontsource-variable/literata";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "@fontsource/ibm-plex-mono/600.css";
import "./styles/globals.css";
import "./styles/cursor.css";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRouter, RouterProvider } from "@tanstack/react-router";
import { platform } from "@tauri-apps/plugin-os";
import { StrictMode, useMemo } from "react";
import ReactDOM from "react-dom/client";
import { createManager } from "tinytick";
import {
  Provider as TinyTickProvider,
  useCreateManager,
} from "tinytick/ui-react";

import "@hypr/ui/globals.css";
import {
  getCurrentWebviewWindowLabel,
  init as initWindowsPlugin,
} from "@hypr/plugin-windows";
import { Toaster } from "@hypr/ui/components/ui/toast";

import { AITaskWindowSyncBridge } from "./ai/task-window-sync";
import { AppI18nProvider } from "./i18n/provider";
import { FloatingMeetingWindowHost } from "./meeting-float/host";
import { routeTree } from "./routeTree.gen";
import { EventListeners } from "./services/event-listeners";
import { LocationInvalidationSync } from "./services/location-invalidation";
import { TaskManager } from "./services/task-manager";
import { RegenerateTranscriptConfirmDialog } from "./session/components/note-input/transcript/regenerate-confirm";
import { useRemoteSessionDeletionUndoListener } from "./session/hooks/useDeleteSession";
import { initializeApplicationSettings } from "./settings/queries";
import { initializeAppExitFlush } from "./shared/app-exit";
import { useConfigValue } from "./shared/config";
import { initConfigStore } from "./shared/config/store";
import { ErrorComponent, NotFoundComponent } from "./shared/control";
import { StartupBoundary } from "./shared/startup-boundary";
import { bootstrapThemeFromSettings } from "./shared/theme/apply";
import { AppThemeProvider } from "./shared/theme/provider";
import type { ThemePreference } from "./shared/theme/resolve";
import { createAITaskStore } from "./store/zustand/ai-task";
import { listenerStore } from "./store/zustand/listener/instance";

const queryClient = new QueryClient();

const router = createRouter({
  routeTree,
  context: undefined,
  defaultErrorComponent: ErrorComponent,
  defaultNotFoundComponent: NotFoundComponent,
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}

function App() {
  const aiTaskStore = useMemo(() => createAITaskStore(), []);

  return (
    <>
      <AITaskWindowSyncBridge store={aiTaskStore} />
      <RegenerateTranscriptConfirmDialog />
      <RouterProvider
        router={router}
        context={{
          listenerStore,
          aiTaskStore,
        }}
      />
    </>
  );
}

function AppRoot() {
  const manager = useCreateManager(() => {
    return createManager().start();
  });
  const theme = useConfigValue("theme") as ThemePreference;
  useRemoteSessionDeletionUndoListener(isMainWindow);

  return (
    <QueryClientProvider client={queryClient}>
      <TinyTickProvider manager={manager}>
        <AppThemeProvider>
          <AppI18nProvider>
            <StartupBoundary>
              <App />
              <LocationInvalidationSync />
              {isMainWindow ? <TaskManager /> : null}
              {isMainWindow ? <FloatingMeetingWindowHost /> : null}
              {isMainWindow ? <EventListeners /> : null}
            </StartupBoundary>
            <Toaster position="bottom-right" theme={theme} />
          </AppI18nProvider>
        </AppThemeProvider>
      </TinyTickProvider>
    </QueryClientProvider>
  );
}

initWindowsPlugin();
document.documentElement.dataset.platform = platform();

const isMainWindow = getCurrentWebviewWindowLabel() === "main";

if (isMainWindow) {
  void initializeAppExitFlush().catch((error) => {
    console.error("Failed to initialize the exit flush listener", error);
  });
}

const rootElement = document.getElementById("root")!;

async function enableReactScanInDev() {
  if (!import.meta.env.DEV) {
    return;
  }

  try {
    const { scan } = await import("react-scan");
    scan({ enabled: true });
  } catch (error) {
    console.warn("Failed to start React Scan:", error);
  }
}

async function renderApp() {
  void initConfigStore().catch((error) => {
    console.error("Failed to initialize the config store", error);
  });
  if (isMainWindow) {
    await initializeApplicationSettings().catch((error) => {
      console.error("Failed to initialize application settings", error);
    });
  }
  await Promise.all([bootstrapThemeFromSettings(), enableReactScanInDev()]);
  const root = ReactDOM.createRoot(rootElement);
  root.render(
    <StrictMode>
      <AppRoot />
    </StrictMode>,
  );
}

if (!rootElement.innerHTML) {
  void renderApp();
}
