export function createAsyncListenerScope() {
  let active = true;
  const cleanups: (() => void)[] = [];
  const cleanup = (fn: () => void) => {
    try {
      fn();
    } catch (error) {
      console.error("[listeners] cleanup failed", error);
    }
  };
  const dispose = () => {
    active = false;
    const pending = cleanups.splice(0);
    pending.forEach(cleanup);
  };
  return {
    get active() {
      return active;
    },
    dispose,
    guard<T>(handler: (event: T) => void) {
      return (event: T) => {
        if (active) handler(event);
      };
    },
    async add(register: () => Promise<() => void>) {
      try {
        if (!active) return;
        const unlisten = await register();
        if (active) cleanups.push(unlisten);
        else cleanup(unlisten);
      } catch (error) {
        dispose();
        throw error;
      }
    },
  };
}
