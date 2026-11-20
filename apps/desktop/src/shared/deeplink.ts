import type { DeepLink } from "@hypr/plugin-deeplink2";

type CommandResult<T> =
  | { status: "ok"; data: T }
  | { status: "error"; error: string };

export async function subscribeThenDrainDeepLinks({
  listen,
  takePendingDeepLinks,
  handle,
}: {
  listen: (handler: (deepLink: DeepLink) => void) => Promise<() => void>;
  takePendingDeepLinks: () => Promise<CommandResult<DeepLink[]>>;
  handle: (deepLink: DeepLink) => void;
}) {
  const drain = async () => {
    let result: CommandResult<DeepLink[]>;
    try {
      result = await takePendingDeepLinks();
    } catch {
      return;
    }
    if (result.status === "ok") {
      for (const deepLink of result.data) {
        handle(deepLink);
      }
    }
  };

  const unlisten = await listen((deepLink) => {
    if (deepLink.to === "/meetings/open") {
      void drain();
    } else {
      handle(deepLink);
    }
  });
  await drain();
  return unlisten;
}
