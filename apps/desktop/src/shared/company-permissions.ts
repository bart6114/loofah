import { createContext, useContext } from "react";

export const CompanyReadOnlyContext = createContext(false);
export const CompanyActiveContext = createContext(false);
export function useCompanyActive() {
  return useContext(CompanyActiveContext);
}
export function useCompanyReadOnly() {
  return useContext(CompanyReadOnlyContext);
}
