import type { IndexEntity } from "./protocol.gen";

import { commands, events } from "~/types/tauri.gen";

type StoreCommand = {
  [K in keyof typeof commands]: K extends
    | `session${string}`
    | `people${string}`
    | `tags${string}`
    | `vault${string}`
    | `enhancedDoc${string}`
    | `transcript${string}`
    ? K
    : never;
}[keyof typeof commands];

export type ApplicationClient = {
  readonly scope: string;
  readonly networkMode: "always" | "online";
  readonly store: Pick<typeof commands, StoreCommand>;
  subscribeIndexChanged: (
    listener: (event: { entity: IndexEntity; ids: string[] }) => void,
  ) => Promise<() => void>;
};

const localClient: ApplicationClient = {
  scope: "local",
  networkMode: "always",
  get store() {
    return commands;
  },
  subscribeIndexChanged: (listener) =>
    events.indexChanged.listen(({ payload }) => listener(payload)),
};

export function getApplicationClient(): ApplicationClient {
  return localClient;
}
