import { Trans } from "@lingui/react/macro";
import { CatchBoundary } from "@tanstack/react-router";
import {
  lazy,
  Suspense,
  useEffect,
  type ComponentType,
  type ReactNode,
} from "react";

export function deferredView<T extends ComponentType<any>>(
  load: () => Promise<{ default: T }>,
) {
  let pending: Promise<{ default: T }> | undefined;
  const preload = () =>
    (pending ??= load().catch((error) => {
      pending = undefined;
      throw error;
    }));
  return { View: lazy(preload), preload };
}

export function usePreloadViews(loaders: readonly (() => Promise<unknown>)[]) {
  useEffect(() => {
    let cancelled = false;
    const preload = async () => {
      for (const load of loaders) {
        if (cancelled) return;
        await load().catch(() => {});
      }
    };
    const frame = requestAnimationFrame(() => {
      timer = setTimeout(() => {
        void preload();
      }, 1000);
    });
    let timer: ReturnType<typeof setTimeout> | undefined;
    return () => {
      cancelled = true;
      cancelAnimationFrame(frame);
      clearTimeout(timer);
    };
  }, [loaders]);
}

export function DeferredView({
  children,
  viewKey,
}: {
  children: ReactNode;
  viewKey: string;
}) {
  return (
    <CatchBoundary
      getResetKey={() => viewKey}
      errorComponent={() => (
        <div role="alert" className="text-muted-foreground p-6">
          <Trans>Unable to open this view. Please try another view.</Trans>
        </div>
      )}
    >
      <Suspense
        fallback={
          <div role="status" className="text-muted-foreground p-6">
            <Trans>Loading…</Trans>
          </div>
        }
      >
        {children}
      </Suspense>
    </CatchBoundary>
  );
}
