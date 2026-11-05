import { QueryClient, queryOptions } from "@tanstack/react-query";

import { ApplicationRequestError } from "./errors";
import type { ApplicationError, Change } from "./protocol.gen";
import type { VaultClient, VaultClientEvent } from "./vault-client";

import type { ConnectionState, SearchRequest } from "~/types/tauri.gen";

export class VaultScope {
  readonly queryClient: QueryClient;
  private snapshot: { state: ConnectionState; error: ApplicationError | null } =
    { state: "connecting", error: null };
  private readonly listeners = new Set<() => void>();
  private readonly changeListeners = new Set<(change: Change) => void>();
  private startPromise?: Promise<void>;
  private unsubscribe?: () => void;
  private disposed = false;

  constructor(readonly client: VaultClient) {
    this.queryClient = new QueryClient({
      defaultOptions: {
        queries: {
          networkMode: client.networkMode,
          staleTime: Infinity,
          retry: (count, error) =>
            count < 2 &&
            (!(error instanceof ApplicationRequestError) ||
              error.detail.retryable),
        },
      },
    });
  }

  getSnapshot = () => this.snapshot;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  onChange(listener: (change: Change) => void) {
    this.changeListeners.add(listener);
    return () => {
      this.changeListeners.delete(listener);
    };
  }

  start(): Promise<void> {
    if (this.disposed)
      return Promise.reject(new Error("This vault scope is closed"));
    return (this.startPromise ??= this.client
      .subscribe(this.receive)
      .then((unsubscribe) => {
        if (this.disposed) unsubscribe();
        else {
          this.unsubscribe = unsubscribe;
          window.addEventListener("focus", this.focus);
          document.addEventListener("visibilitychange", this.visible);
        }
      })
      .catch((error: unknown) => {
        this.startPromise = undefined;
        if (!this.disposed) {
          this.snapshot = {
            state: "reconnecting",
            error:
              error instanceof ApplicationRequestError ? error.detail : null,
          };
          for (const listener of this.listeners) listener();
        }
        throw error;
      }));
  }

  noteOptions(sessionId: string) {
    const client = this.client;
    return queryOptions({
      queryKey: ["vault", client.scope, "note", sessionId],
      queryFn: ({ signal }) => client.readNote(sessionId, { signal }),
    });
  }
  sessionOptions(sessionId: string) {
    const client = this.client;
    return queryOptions({
      queryKey: ["vault", client.scope, "session", sessionId],
      queryFn: ({ signal }) => client.session(sessionId, { signal }),
    });
  }
  pageOptions(limit: number, after: string | null = null) {
    const client = this.client;
    return queryOptions({
      queryKey: ["vault", client.scope, "session_page", limit, after],
      queryFn: ({ signal }) => client.sessions(limit, after, { signal }),
    });
  }
  searchOptions(input: SearchRequest) {
    const client = this.client;
    return queryOptions({
      queryKey: ["vault", client.scope, "search", input],
      queryFn: ({ signal }) => client.search(input, { signal }),
    });
  }

  resync() {
    if (!this.disposed)
      void this.queryClient.invalidateQueries({
        queryKey: ["vault", this.client.scope],
      });
  }

  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    this.unsubscribe?.();
    window.removeEventListener("focus", this.focus);
    document.removeEventListener("visibilitychange", this.visible);
    void this.queryClient.cancelQueries();
    this.queryClient.clear();
    this.snapshot = { state: "closed", error: null };
    for (const listener of this.listeners) listener();
    this.listeners.clear();
    this.changeListeners.clear();
  }

  private focus = () => {
    if (["connected", "reconnecting"].includes(this.snapshot.state))
      this.resync();
  };
  private visible = () => {
    if (document.visibilityState === "visible") this.focus();
  };

  private receive = (event: VaultClientEvent) => {
    if (this.disposed) return;
    if (event.kind === "connection") {
      const previous = this.snapshot.state;
      this.snapshot = { state: event.state, error: event.error };
      if (
        ["access_denied", "login_required", "closed", "incompatible"].includes(
          event.state,
        )
      ) {
        void this.queryClient.cancelQueries();
        this.queryClient.clear();
      } else if (event.state === "connected" && previous === "reconnecting")
        this.resync();
      for (const listener of this.listeners) listener();
      return;
    }
    const change = event.change;
    for (const listener of this.changeListeners) listener(change);
    if (change.kind === "resync_required" || change.kind === "unknown")
      this.resync();
    else if (change.kind === "search_ready") {
      void this.queryClient.invalidateQueries({
        queryKey: ["vault", this.client.scope, "search"],
      });
    } else if (change.kind === "index_changed") {
      const ids = new Set(change.ids);
      const all = ids.size === 0;
      void this.queryClient.invalidateQueries({
        predicate: (query) => {
          const [prefix, scope, kind, id] = query.queryKey;
          if (prefix !== "vault" || scope !== this.client.scope) return false;
          if (kind === "session_page")
            return ["sessions", "session_headers"].includes(change.entity);
          if (kind === "note" || kind === "session")
            return (
              ["sessions", "session_headers", "tasks"].includes(
                change.entity,
              ) &&
              (all || ids.has(String(id)))
            );
          return kind === change.entity && (all || ids.has(String(id)));
        },
      });
    }
  };
}
