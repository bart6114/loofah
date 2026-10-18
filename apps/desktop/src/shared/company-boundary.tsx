import { Trans } from "@lingui/react/macro";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";

import { CompanyStorageSettings } from "~/settings/general/storage/company";
import {
  CompanyActiveContext,
  CompanyReadOnlyContext,
} from "~/shared/company-permissions";
import {
  companyRequest,
  type CompanyVault,
  useCompanyStatus,
} from "~/shared/company-vault";

export function CompanyBoundary({ children }: { children: React.ReactNode }) {
  const queryClient = useQueryClient();
  const status = useCompanyStatus();
  const vaults = useQuery({
    queryKey: [
      "company",
      status.data?.profile?.server_id,
      status.data?.profile?.user_id,
      "vaults",
    ],
    queryFn: () => companyRequest<CompanyVault[]>("GET", "/v1/vaults"),
    enabled: Boolean(status.data?.active),
    retry: false,
  });
  const role = vaults.data?.find(
    (vault) => vault.id === status.data?.profile?.vault_id,
  )?.members[status.data?.profile?.user_id ?? ""];
  const readOnly = Boolean(
    status.data?.active && role !== "owner" && role !== "editor",
  );
  const [revoked, setRevoked] = useState(false);
  useEffect(() => {
    const subscriptions = [
      listen("company-authenticated", () => {
        setRevoked(false);
        void queryClient.invalidateQueries({ queryKey: ["company"] });
      }),
      listen("company-status-changed", () => {
        void queryClient.invalidateQueries({ queryKey: ["company", "status"] });
      }),
      listen("company-vault-list-changed", () => {
        void queryClient.invalidateQueries({
          predicate: (query) =>
            query.queryKey[0] === "company" &&
            (query.queryKey.includes("vaults") ||
              query.queryKey.includes("users")),
        });
      }),
      listen("company-vault-changed", () => {
        queryClient.clear();
        window.location.reload();
      }),
      listen("company-access-revoked", () => {
        setRevoked(true);
        void queryClient.cancelQueries();
        queryClient.clear();
      }),
    ];
    return () => {
      for (const subscription of subscriptions)
        void subscription.then((unlisten) => unlisten()).catch(() => {});
    };
  }, [queryClient]);
  if (revoked)
    return (
      <div className="bg-background fixed inset-0 z-50 overflow-auto p-10">
        <div className="mx-auto max-w-xl space-y-4">
          <h1 className="text-lg font-semibold">
            <Trans>Company vault access is unavailable</Trans>
          </h1>
          <p className="text-muted-foreground text-sm">
            <Trans>
              Sign in again or choose your local folder. Unsent work remains
              available for recovery.
            </Trans>
          </p>
          <CompanyStorageSettings />
        </div>
      </div>
    );
  const pending =
    (status.data?.pending.length ?? 0) + (status.data?.transfers.length ?? 0);
  return (
    <CompanyActiveContext.Provider value={Boolean(status.data?.active)}>
      <CompanyReadOnlyContext.Provider value={readOnly}>
        {children}
        {status.data?.active && (
          <div
            role="status"
            className="bg-background/95 text-muted-foreground pointer-events-none fixed right-4 bottom-3 z-40 rounded border px-3 py-1 text-xs"
          >
            {status.data.error
              ? status.data.error.message
              : pending
                ? `Saving to company vault · ${pending} pending`
                : readOnly
                  ? "Read-only company vault"
                  : "Saved to company vault"}
          </div>
        )}
      </CompanyReadOnlyContext.Provider>
    </CompanyActiveContext.Provider>
  );
}
