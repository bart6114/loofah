import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import { ApplicationRequestError } from "./errors";
import type {
  ProcessingPolicy,
  ProviderCredentialSaveOutcome,
  ProviderCredentialStatus,
} from "./protocol.gen";
import type { NativeRemoteConnection } from "./remote";
import {
  connectRemoteVault,
  type VaultClient,
  type VaultClientEvent,
} from "./vault-client";
import { VaultProcessingSettings } from "./vault-processing-settings";
import { VaultProviderCredentials } from "./vault-provider-credentials";
import { VaultScope } from "./vault-scope";

const scopes: VaultScope[] = [];
const initialRevision = "a99d4f53-4db4-4460-9b09-dc1606d5f260";
const savedRevision = "a99d4f53-4db4-4460-9b09-dc1606d5f261";
const canonicalRevision = "a99d4f53-4db4-4460-9b09-dc1606d5f262";
const policy: ProcessingPolicy = {
  allow_custom_templates: false,
  providers: [
    {
      id: "openai",
      models: ["approved"],
      base_url: "https://api.openai.com/v1",
      allow_custom_model: false,
      allow_custom_endpoint: false,
      requires_key: true,
    },
  ],
};

afterEach(() => {
  cleanup();
  for (const scope of scopes.splice(0)) scope.dispose();
  vi.restoreAllMocks();
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

async function fixture({
  credentials = true,
  canEdit = true,
  scopeId = "credentials-test",
} = {}) {
  let receive!: (event: VaultClientEvent) => void;
  let current: ProviderCredentialStatus = {
    provider_id: "openai",
    revision: initialRevision,
    source: "operator",
    endpoint_matches: true,
  };
  const status = vi.fn<
    NonNullable<VaultClient["providerCredentials"]>["status"]
  >(async () => structuredClone(current));
  const save = vi.fn<NonNullable<VaultClient["providerCredentials"]>["save"]>(
    async (_provider, input) => {
      current = {
        ...current,
        revision: savedRevision,
        source: input.api_key ? "vault" : "operator",
      };
      return { kind: "saved", status: structuredClone(current) };
    },
  );
  const scope = new VaultScope({
    scope: scopeId,
    canEdit,
    features: ["processing_settings", "provider_credentials"],
    networkMode: "always",
    providerCredentials: credentials ? { status, save } : undefined,
    processingPolicy: async () => policy,
    processingSettings: async () => ({
      revision: "a".repeat(64),
      settings: {
        summary_model: null,
        ai_language: "en",
        summary_prompt: "",
        auto_apply_high_confidence_tags: true,
      },
    }),
    subscribe: async (callback: (event: VaultClientEvent) => void) => {
      receive = callback;
      callback({ kind: "connection", state: "connected", error: null });
      return () => {};
    },
  } as unknown as VaultClient);
  scopes.push(scope);
  scope.queryClient.setDefaultOptions({
    queries: { retry: false, staleTime: Infinity, networkMode: "always" },
    mutations: { retry: false, networkMode: "always" },
  });
  await scope.start();
  return {
    scope,
    status,
    save,
    emit: (event: VaultClientEvent) => receive(event),
    setCurrent: (value: Partial<ProviderCredentialStatus>) => {
      current = { ...current, ...value };
    },
  };
}

function input() {
  return screen.getByLabelText("New API key") as HTMLInputElement;
}
function typeKey(secret = "PRIVATE-CANARY") {
  fireEvent.change(input(), { target: { value: secret } });
}
function submit() {
  fireEvent.click(screen.getByRole("button", { name: "Save vault key" }));
}
async function ready() {
  await waitFor(() => expect(input().matches(":disabled")).toBe(false));
}
async function openSettings(scope: VaultScope) {
  const mounted = render(<VaultProcessingSettings scope={scope} />);
  fireEvent.click(screen.getByRole("button", { name: "Processing settings" }));
  await ready();
  return mounted;
}

it.each(["Local", "reader", "older server"])(
  "hides the credential form when %s exposes no owner credential API",
  async (kind) => {
    const backend = await fixture({
      credentials: false,
      canEdit: kind !== "reader",
    });
    render(
      <VaultProviderCredentials
        scope={backend.scope}
        policy={policy}
        connected
      />,
    );
    expect(
      screen.queryByRole("region", { name: "Provider credentials" }),
    ).toBeNull();
    expect(screen.queryByLabelText("New API key")).toBeNull();
    expect(backend.status).not.toHaveBeenCalled();
    expect(backend.save).not.toHaveBeenCalled();
  },
);

it("shows inactive endpoint status without rendering any saved credential", async () => {
  const backend = await fixture();
  backend.setCurrent({ endpoint_matches: false, source: "operator" });
  render(
    <VaultProviderCredentials
      scope={backend.scope}
      policy={policy}
      connected
    />,
  );
  await ready();
  expect(screen.getByText(/previous vault key is inactive/)).toBeDefined();
  expect(input().value).toBe("");
  expect(input().type).toBe("password");
  expect(input().autocomplete).toBe("new-password");
  expect(input().maxLength).toBe(16384);
});

it("clears the visible secret before dispatch and removes it from settled mutation variables", async () => {
  const backend = await fixture();
  const pending = deferred<ProviderCredentialSaveOutcome>();
  let visibleAtDispatch: string | undefined;
  backend.save.mockImplementationOnce(async () => {
    visibleAtDispatch = input().value;
    return pending.promise;
  });
  render(
    <VaultProviderCredentials
      scope={backend.scope}
      policy={policy}
      connected
    />,
  );
  await ready();
  typeKey();
  submit();
  await waitFor(() => expect(backend.save).toHaveBeenCalledOnce());
  expect(visibleAtDispatch).toBe("");
  expect(input().value).toBe("");
  expect(input().matches(":disabled")).toBe(true);
  expect(backend.save.mock.calls[0][1]).toEqual({
    expected_revision: initialRevision,
    api_key: "PRIVATE-CANARY",
  });
  const requestId = backend.save.mock.calls[0][2]?.requestId;
  expect(requestId).toMatch(/^[\da-f]{8}-[\da-f-]{27}$/);
  backend.setCurrent({ revision: canonicalRevision, source: "vault" });
  await act(async () =>
    pending.resolve({
      kind: "saved",
      status: {
        provider_id: "openai",
        revision: savedRevision,
        source: "vault",
        endpoint_matches: true,
      },
    }),
  );
  await ready();
  expect(backend.status).toHaveBeenCalledTimes(2);
  expect(
    JSON.stringify(
      backend.scope.queryClient
        .getMutationCache()
        .getAll()
        .map(
          (mutation) =>
            (mutation.state.variables as { input?: unknown })?.input,
        ),
    ),
  ).not.toContain("PRIVATE-CANARY");
  expect(screen.getByText("Credential update confirmed.")).toBeDefined();
});

it("clears failed submissions and refreshes status without replaying the secret update", async () => {
  const backend = await fixture();
  backend.save.mockRejectedValueOnce(
    new Error("Connection lost; update outcome unknown"),
  );
  render(
    <VaultProviderCredentials
      scope={backend.scope}
      policy={policy}
      connected
    />,
  );
  await ready();
  typeKey();
  submit();
  await screen.findByText("Connection lost; update outcome unknown");
  expect(input().value).toBe("");
  await waitFor(() => expect(backend.status).toHaveBeenCalledTimes(2));
  expect(backend.save).toHaveBeenCalledOnce();
  fireEvent.click(
    screen.getByRole("button", { name: "Refresh credential status" }),
  );
  await waitFor(() => expect(backend.status).toHaveBeenCalledTimes(3));
  expect(backend.save).toHaveBeenCalledOnce();
  expect(input().value).toBe("");
  expect(
    JSON.stringify(
      backend.scope.queryClient
        .getMutationCache()
        .getAll()
        .map(
          (mutation) =>
            (mutation.state.variables as { input?: unknown })?.input,
        ),
    ),
  ).not.toContain("PRIVATE-CANARY");
});

it("refreshes canonical status after conflict and requires a newly entered key against its revision", async () => {
  const backend = await fixture();
  backend.save.mockImplementationOnce(async () => {
    backend.setCurrent({ revision: canonicalRevision, source: "missing" });
    return {
      kind: "conflict",
      status: {
        provider_id: "openai",
        revision: savedRevision,
        source: "vault",
        endpoint_matches: true,
      },
    };
  });
  render(
    <VaultProviderCredentials
      scope={backend.scope}
      policy={policy}
      connected
    />,
  );
  await ready();
  typeKey();
  submit();
  await screen.findByText(
    "The credential changed. Review its status before saving another key.",
  );
  await screen.findByText("No provider key is configured.");
  await ready();
  expect(input().value).toBe("");
  submit();
  expect(backend.save).toHaveBeenCalledOnce();
  typeKey("EXPLICIT-NEW-KEY");
  submit();
  await waitFor(() => expect(backend.save).toHaveBeenCalledTimes(2));
  expect(backend.save.mock.calls[1][1]).toEqual({
    expected_revision: canonicalRevision,
    api_key: "EXPLICIT-NEW-KEY",
  });
});

it("clears only after explicit action and never sends a typed replacement with a clear request", async () => {
  const backend = await fixture();
  backend.setCurrent({ source: "vault" });
  render(
    <VaultProviderCredentials
      scope={backend.scope}
      policy={policy}
      connected
    />,
  );
  await ready();
  typeKey("UNSENT-KEY");
  fireEvent.click(screen.getByRole("button", { name: "Clear vault key" }));
  await waitFor(() => expect(backend.save).toHaveBeenCalledOnce());
  expect(backend.save.mock.calls[0][1]).toEqual({
    expected_revision: initialRevision,
    api_key: null,
  });
  expect(input().value).toBe("");
  await screen.findByText("Using the server's configured key.");
});

it("disables credential writes during disconnect and tears down the dialog and cache on access loss", async () => {
  const backend = await fixture();
  await openSettings(backend.scope);
  typeKey();
  act(() =>
    backend.emit({ kind: "connection", state: "reconnecting", error: null }),
  );
  expect(input().matches(":disabled")).toBe(true);
  submit();
  fireEvent.click(screen.getByRole("button", { name: "Clear vault key" }));
  expect(backend.save).not.toHaveBeenCalled();
  act(() =>
    backend.emit({ kind: "connection", state: "access_denied", error: null }),
  );
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(screen.queryByLabelText("New API key")).toBeNull();
  expect(backend.scope.queryClient.getQueryCache().getAll()).toHaveLength(0);
});

it("forbidden saves close private settings and cannot restore the old credential status", async () => {
  const backend = await fixture();
  backend.save.mockRejectedValueOnce(
    new ApplicationRequestError({
      code: "forbidden",
      message: "Owner access revoked",
      retryable: false,
      request_id: null,
      current_revision: null,
      mutation_outcome: "not_committed",
    }),
  );
  await openSettings(backend.scope);
  typeKey();
  submit();
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(backend.scope.getSnapshot().state).toBe("access_denied");
  expect(backend.scope.queryClient.getQueryCache().getAll()).toHaveLength(0);
  expect(backend.save).toHaveBeenCalledOnce();
  expect(backend.status).toHaveBeenCalledOnce();
});

it("remounts same-string scopes and never sends a draft or delayed old save to the replacement owner", async () => {
  const old = await fixture({ scopeId: "same-scope" });
  const replacement = await fixture({ scopeId: "same-scope" });
  replacement.setCurrent({ revision: canonicalRevision, source: "missing" });
  const pending = deferred<ProviderCredentialSaveOutcome>();
  old.save.mockReturnValueOnce(pending.promise);
  const mounted = render(
    <VaultProviderCredentials scope={old.scope} policy={policy} connected />,
  );
  await ready();
  typeKey("OLD-OWNER-KEY");
  submit();
  await waitFor(() => expect(old.save).toHaveBeenCalledOnce());
  old.scope.dispose();
  mounted.rerender(
    <VaultProviderCredentials
      scope={replacement.scope}
      policy={policy}
      connected
    />,
  );
  await ready();
  expect(input().value).toBe("");
  typeKey("REPLACEMENT-DRAFT");
  await act(async () =>
    pending.resolve({
      kind: "saved",
      status: {
        provider_id: "openai",
        revision: savedRevision,
        source: "vault",
        endpoint_matches: true,
      },
    }),
  );
  expect(input().value).toBe("REPLACEMENT-DRAFT");
  expect(screen.queryByText("Credential update confirmed.")).toBeNull();
  expect(screen.getByText("No provider key is configured.")).toBeDefined();
  expect(replacement.save).not.toHaveBeenCalled();
  expect(replacement.status).toHaveBeenCalledOnce();
  expect(old.scope.queryClient.getQueryCache().getAll()).toHaveLength(0);
  submit();
  await waitFor(() => expect(replacement.save).toHaveBeenCalledOnce());
  expect(replacement.save.mock.calls[0][1]).toEqual({
    expected_revision: canonicalRevision,
    api_key: "REPLACEMENT-DRAFT",
  });
  expect(old.save).toHaveBeenCalledOnce();
});

it("ignores a late status read from a retired same-string scope", async () => {
  const old = await fixture({ scopeId: "same-scope" });
  const replacement = await fixture({ scopeId: "same-scope" });
  replacement.setCurrent({ revision: canonicalRevision, source: "missing" });
  const pending = deferred<ProviderCredentialStatus>();
  old.status.mockReturnValueOnce(pending.promise);
  const mounted = render(
    <VaultProviderCredentials scope={old.scope} policy={policy} connected />,
  );
  await waitFor(() => expect(old.status).toHaveBeenCalledOnce());
  old.scope.dispose();
  mounted.rerender(
    <VaultProviderCredentials
      scope={replacement.scope}
      policy={policy}
      connected
    />,
  );
  await ready();
  await act(async () =>
    pending.resolve({
      provider_id: "openai",
      revision: savedRevision,
      source: "vault",
      endpoint_matches: true,
    }),
  );
  expect(screen.getByText("No provider key is configured.")).toBeDefined();
  expect(old.scope.queryClient.getQueryCache().getAll()).toHaveLength(0);
  typeKey();
  submit();
  await waitFor(() => expect(replacement.save).toHaveBeenCalledOnce());
  expect(replacement.save.mock.calls[0][1].expected_revision).toBe(
    canonicalRevision,
  );
});

it("exposes the remote credential adapter only to an owner on a capable server", async () => {
  const request = vi.fn(async () => ({
    provider_id: "openai",
    revision: initialRevision,
    source: "operator",
    endpoint_matches: true,
  }));
  const connection = (features: string[]) =>
    ({
      info: { server: "https://company.example/", capabilities: { features } },
      request,
    }) as unknown as NativeRemoteConnection;
  for (const role of ["reader", "editor"] as const) {
    expect(
      connectRemoteVault(connection(["provider_credentials"]), "member", {
        id: "vault",
        role,
      }).providerCredentials,
    ).toBeUndefined();
  }
  expect(
    connectRemoteVault(connection([]), "member", { id: "vault", role: "owner" })
      .providerCredentials,
  ).toBeUndefined();
  expect(request).not.toHaveBeenCalled();
  const owner = connectRemoteVault(
    connection(["provider_credentials"]),
    "member",
    { id: "original-vault", role: "owner" },
  );
  await owner.providerCredentials!.status("openai");
  expect(request).toHaveBeenCalledWith(
    {
      kind: "provider_credential_status",
      vault_id: "original-vault",
      provider_id: "openai",
    },
    "provider_credential_status",
    undefined,
  );
});
