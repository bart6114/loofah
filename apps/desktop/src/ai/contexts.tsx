import React, { createContext, useContext, useEffect, useRef } from "react";
import { shallow } from "zustand/shallow";
import { useStoreWithEqualityFn } from "zustand/traditional";

import { type AITaskStore, createAITaskStore } from "~/store/zustand/ai-task";

const AITaskContext = createContext<AITaskStore | null>(null);

export type AITaskState = ReturnType<
  ReturnType<typeof createAITaskStore>["getState"]
>;

export const AITaskProvider = ({
  children,
  store,
}: {
  children: React.ReactNode;
  store: AITaskStore;
}) => {
  const storeRef = useRef<AITaskStore | null>(null);
  if (!storeRef.current) {
    storeRef.current = store;
  }

  useEffect(() => storeRef.current!.getState().startHistoryMaintenance(), []);

  return (
    <AITaskContext.Provider value={storeRef.current}>
      {children}
    </AITaskContext.Provider>
  );
};

export const useAITask = <T,>(
  selector: (state: AITaskState) => T,
  equalityFn?: (left: T, right: T) => boolean,
) => {
  const store = useContext(AITaskContext);

  if (!store) {
    throw new Error("'useAITask' must be used within a 'AITaskProvider'");
  }

  return useStoreWithEqualityFn(store, selector, equalityFn ?? shallow);
};
