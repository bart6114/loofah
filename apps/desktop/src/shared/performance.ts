const enabled = import.meta.env.VITE_PROFILE === "true";
const LIMIT = 2000;
const samples = new Map<string, number[]>();

function record(name: string, value: number) {
  const values = samples.get(name) ?? [];
  if (values.length === LIMIT) values.shift();
  values.push(value);
  samples.set(name, values);
}

export async function measureLocalQuery<T>(
  operation: () => Promise<T>,
): Promise<T> {
  if (!enabled) return operation();
  const start = performance.now();
  try {
    return await operation();
  } finally {
    record("vault-query-ms", performance.now() - start);
  }
}

export function startPerformanceDiagnostics() {
  if (!enabled) return;
  const report = () =>
    Object.fromEntries(
      [...samples].map(([name, values]) => {
        const sorted = [...values].sort((a, b) => a - b);
        return [
          name,
          {
            samples: sorted.length,
            p50: sorted[Math.floor(sorted.length * 0.5)],
            p95: sorted[
              Math.min(sorted.length - 1, Math.floor(sorted.length * 0.95))
            ],
            max: sorted[sorted.length - 1],
          },
        ];
      }),
    );
  Object.assign(window, {
    __LOOFAH_PERFORMANCE__: { report, reset: () => samples.clear() },
  });
  let previous = 0;
  const frame = (now: number) => {
    if (document.visibilityState === "visible") {
      if (previous) record("frame-interval-ms", now - previous);
      previous = now;
    } else previous = 0;
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);
  const onInput = () => {
    const start = performance.now();
    requestAnimationFrame(() =>
      record("input-to-frame-ms", performance.now() - start),
    );
  };
  for (const event of ["pointerdown", "keydown"])
    document.addEventListener(event, onInput, { capture: true, passive: true });
}
