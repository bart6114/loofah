import { useTabs } from "~/store/zustand/tabs";

export function useOpenSummaryPrompt() {
  const openNew = useTabs((state) => state.openNew);
  return () => {
    openNew({ type: "settings", state: { tab: "summary-prompt" } });
  };
}
