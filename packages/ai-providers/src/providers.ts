import definitions from "./providers.json";
export type ProviderRequirement =
  | { kind: "requires_auth" }
  | { kind: "requires_entitlement"; entitlement: "pro" }
  | { kind: "requires_config"; fields: Array<"base_url" | "api_key"> }
  | { kind: "requires_platform"; platform: "apple_silicon" };

export type ProviderDefinition = {
  id: string;
  displayName: string;
  badge: string | null;
  baseUrl?: string;
  requirements: ProviderRequirement[];
  mobileSupported: boolean;
  links?: {
    download?: { label: string; url: string };
    models?: { label: string; url: string };
    setup?: { label: string; url: string };
  };
};
export type ProviderId =
  | "lmstudio"
  | "ollama"
  | "openrouter"
  | "openai"
  | "chatgpt_subscription"
  | "cloudflare_workers_ai"
  | "anthropic"
  | "mistral"
  | "azure_openai"
  | "azure_ai"
  | "google_generative_ai"
  | "custom";
export const PROVIDERS = definitions as Array<
  ProviderDefinition & { id: ProviderId }
>;
