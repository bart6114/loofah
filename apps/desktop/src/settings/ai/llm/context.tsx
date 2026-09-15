import { createContext, useContext, useState } from "react";

type LlmSettingsContextType = {
  accordionValue: string;
  setAccordionValue: (value: string) => void;
  editingConnection: boolean;
  setEditingConnection: (value: boolean) => void;
  connectionProvider: string;
  setConnectionProvider: (value: string) => void;
};

const LlmSettingsContext = createContext<LlmSettingsContextType | null>(null);

export function LlmSettingsProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const [accordionValue, setAccordionValue] = useState<string>("");

  const [editingConnection, setEditingConnection] = useState(false);
  const [connectionProvider, setConnectionProvider] = useState("");

  return (
    <LlmSettingsContext.Provider
      value={{
        accordionValue,
        setAccordionValue,
        editingConnection,
        setEditingConnection,
        connectionProvider,
        setConnectionProvider,
      }}
    >
      {children}
    </LlmSettingsContext.Provider>
  );
}

export function useLlmSettings() {
  const context = useContext(LlmSettingsContext);
  if (!context) {
    throw new Error("useLlmSettings must be used within LlmSettingsProvider");
  }
  return context;
}
