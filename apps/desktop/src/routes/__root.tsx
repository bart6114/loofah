import { createRootRouteWithContext } from "@tanstack/react-router";
import { lazy, Suspense } from "react";

import type { Context } from "~/types";

const MainAppLayout = lazy(() => import("~/shared/main-app-layout"));

export const Route = createRootRouteWithContext<Partial<Context>>()({
  component: Component,
});

function Component() {
  return (
    <Suspense fallback={<div className="bg-background h-screen w-screen" />}>
      <MainAppLayout />
    </Suspense>
  );
}
