import { applicationResult } from "./errors";

import {
  flushDatabaseWritesByPrefix,
  enqueueDatabaseWrite,
} from "~/shared/write-queue";
import { commands, type NoteDraftRecovery } from "~/types/tauri.gen";

export class NativeDraftStore {
  private readonly prefix: string;
  constructor(readonly scope: string) {
    this.prefix = `draft:${JSON.stringify(scope)}:`;
  }

  async inventory() {
    await this.flush();
    return applicationResult(await commands.draftCacheInventory(this.scope));
  }

  async recovery(id: string): Promise<NoteDraftRecovery | null> {
    await this.flush();
    const value = applicationResult(
      await commands.draftCacheRecovery(this.scope, id),
    );
    if (value && value.scope !== this.scope)
      throw new Error(
        "This recovery draft belongs to a different account or vault",
      );
    return value;
  }

  async load(sessionId: string): Promise<NoteDraftRecovery | null> {
    await this.flush();
    return applicationResult(
      await commands.draftCacheGet(this.scope, sessionId),
    );
  }

  save(recovery: NoteDraftRecovery): Promise<void> {
    if (recovery.scope !== this.scope)
      return Promise.reject(
        new Error(
          "This recovery draft belongs to a different account or vault",
        ),
      );
    const captured = {
      ...recovery,
      baseline: { ...recovery.baseline },
      draft: { ...recovery.draft },
    };
    return enqueueDatabaseWrite(this.key(captured.sessionId), async () => {
      applicationResult(await commands.draftCachePut(captured));
    });
  }

  remove(sessionId: string): Promise<void> {
    return enqueueDatabaseWrite(this.key(sessionId), async () => {
      applicationResult(await commands.draftCacheRemove(this.scope, sessionId));
    });
  }

  flush(): Promise<void> {
    return flushDatabaseWritesByPrefix([this.prefix]);
  }
  private key(sessionId: string): string {
    return `${this.prefix}${JSON.stringify(sessionId)}`;
  }
}
