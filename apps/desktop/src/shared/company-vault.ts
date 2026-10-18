import { useQuery } from "@tanstack/react-query";
import { invoke, isTauri } from "@tauri-apps/api/core";

export type CompanyRole = "owner" | "editor" | "viewer";
export type CompanyVault = {
  id: string;
  name: string;
  personal: boolean;
  members: Record<string, CompanyRole>;
};
export type CompanyUser = { id: string; name: string; email: string };
export type PendingCompanyDraft = {
  call: {
    mutation_id: string;
    operation: string;
    args: Record<string, unknown>;
  };
  error: { code: string; message: string } | null;
};
export type CompanyStatus = {
  active: boolean;
  recording: boolean;
  processing: boolean;
  profile: {
    base_url: string;
    server_id: string;
    user_id: string;
    vault_id: string | null;
  } | null;
  error: { code: string; message: string } | null;
  pending: PendingCompanyDraft[];
  transfers: {
    id: string;
    session_id: string;
    filename: string;
    error: { message: string } | null;
  }[];
};

export function companyAction<T>(
  operation: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  return invoke<T>("company_action", { operation, args });
}

export function companyRequest<T>(
  method: string,
  path: string,
  body?: unknown,
): Promise<T> {
  return companyAction<T>("request", {
    method,
    path,
    ...(body === undefined ? {} : { body }),
  });
}

export function useCompanyStatus() {
  return useQuery({
    queryKey: ["company", "status"],
    queryFn: () => companyAction<CompanyStatus>("status"),
    refetchInterval: 5000,
    retry: false,
  });
}

export async function beginVaultProcessing(): Promise<
  (() => Promise<void>) | undefined
> {
  if (typeof isTauri !== "function" || !isTauri()) return;
  const id = await companyAction<string>("begin_processing");
  return async () => {
    await companyAction("end_processing", { id });
  };
}
