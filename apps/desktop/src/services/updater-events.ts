import {
  commands as updaterCommands,
  events as updaterEvents,
} from "@hypr/plugin-updater2";
import { getCurrentWebviewWindowLabel } from "@hypr/plugin-windows";

import { createAsyncListenerScope } from "~/shared/async-listener-scope";
import { useLatestRef } from "~/shared/hooks/useLatestRef";
import { useMountEffect } from "~/shared/hooks/useMountEffect";
import { useTabs } from "~/store/zustand/tabs";

export function useUpdaterEvents() {
  const openNew = useTabs((state) => state.openNew);
  const openNewRef = useLatestRef(openNew);

  useMountEffect(() => {
    if (getCurrentWebviewWindowLabel() !== "main") {
      return;
    }

    const scope = createAsyncListenerScope();

    void scope
      .add(() =>
        updaterEvents.updatedEvent.listen(
          scope.guard(({ payload: { previous, current } }) => {
            openNewRef.current({
              type: "changelog",
              state: { previous, current },
            });
          }),
        ),
      )
      .then(async () => {
        if (scope.active) await updaterCommands.maybeEmitUpdated();
      })
      .catch((error) =>
        console.error("[updater] listener setup failed", error),
      );

    return scope.dispose;
  });
}
