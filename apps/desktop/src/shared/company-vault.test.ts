import { beforeEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), isTauri: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => mocks);
import {
  beginVaultProcessing,
  companyAction,
  companyRequest,
} from "./company-vault";

beforeEach(() => {
  vi.resetAllMocks();
});

it("passes typed company operations through native IPC", async () => {
  mocks.invoke.mockResolvedValue({ accepted: true });
  expect(await companyAction("retry")).toEqual({ accepted: true });
  expect(mocks.invoke).toHaveBeenCalledWith("company_action", {
    operation: "retry",
    args: {},
  });
  await companyRequest("GET", "/v1/vaults");
  expect(mocks.invoke).toHaveBeenLastCalledWith("company_action", {
    operation: "request",
    args: { method: "GET", path: "/v1/vaults" },
  });
  await companyRequest("POST", "/v1/vaults", { name: "Team" });
  expect(mocks.invoke).toHaveBeenLastCalledWith("company_action", {
    operation: "request",
    args: { method: "POST", path: "/v1/vaults", body: { name: "Team" } },
  });
});

it("does not acquire a processing lease outside Tauri", async () => {
  mocks.isTauri.mockReturnValue(false);
  expect(await beginVaultProcessing()).toBeUndefined();
  expect(mocks.invoke).not.toHaveBeenCalled();
});

it("holds processing to the native lease until released", async () => {
  mocks.isTauri.mockReturnValue(true);
  mocks.invoke
    .mockResolvedValueOnce("lease-id")
    .mockResolvedValueOnce(undefined);
  const release = await beginVaultProcessing();
  expect(mocks.invoke).toHaveBeenCalledExactlyOnceWith("company_action", {
    operation: "begin_processing",
    args: {},
  });
  await release!();
  expect(mocks.invoke).toHaveBeenLastCalledWith("company_action", {
    operation: "end_processing",
    args: { id: "lease-id" },
  });
});

it("propagates lease errors so processing cannot silently continue in another vault", async () => {
  mocks.isTauri.mockReturnValue(true);
  mocks.invoke.mockRejectedValueOnce(new Error("Vault is switching"));
  await expect(beginVaultProcessing()).rejects.toThrow("Vault is switching");
});
