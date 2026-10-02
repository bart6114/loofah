import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { SummaryProviderBridge } from "./SummaryProviderBridge";
const client = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 10_000, retry: 1 },
    mutations: { retry: false },
  },
});
createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={client}>
    <SummaryProviderBridge />
    <App />
  </QueryClientProvider>,
);
