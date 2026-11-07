import type { MessageDescriptor } from "@lingui/core";
import { msg } from "@lingui/core/macro";
import { convertFileSrc } from "@tauri-apps/api/core";
import { join } from "@tauri-apps/api/path";

import { commands as fsSyncCommands } from "@hypr/plugin-fs-sync";
import { commands as openerCommands } from "@hypr/plugin-opener2";

import { applicationResult, connectionClosed } from "./errors";
import type { FileInfo } from "./protocol.gen";
import type { NativeRemoteConnection } from "./remote";
import type { RequestOptions } from "./vault-client";

import { commands } from "~/types/tauri.gen";

export type AttachmentResource = {
  readonly id: string;
  readonly info: FileInfo;
  readonly src?: string;
  readonly path?: string;
  readonly labels: {
    open: MessageDescriptor;
    reveal: MessageDescriptor;
    save?: MessageDescriptor;
  };
  open: (options?: RequestOptions) => Promise<unknown>;
  reveal: (options?: RequestOptions) => Promise<unknown>;
  save?: (options?: RequestOptions) => Promise<unknown>;
  close: () => Promise<void>;
};

export async function openLocalAttachment(
  sessionId: string,
  attachmentId: string,
  options?: RequestOptions,
): Promise<AttachmentResource | null> {
  options?.signal?.throwIfAborted();
  const info = applicationResult(
    await commands.applicationAttachmentInfo(sessionId, attachmentId),
  );
  options?.signal?.throwIfAborted();
  if (!info) return null;
  const directory = await fsSyncCommands.attachmentDir(sessionId);
  if (directory.status === "error") throw new Error(directory.error);
  options?.signal?.throwIfAborted();
  const path = await join(directory.data, info.filename);
  options?.signal?.throwIfAborted();
  let closed = false;
  const check = (request?: RequestOptions) => {
    if (closed) throw connectionClosed();
    request?.signal?.throwIfAborted();
  };
  return {
    id: crypto.randomUUID(),
    info,
    path,
    src: info.contentType.startsWith("image/")
      ? convertFileSrc(path)
      : undefined,
    labels: { open: msg`Open file`, reveal: msg`Show file` },
    open: async (request) => {
      check(request);
      await openerCommands.openPath(path, null);
    },
    reveal: async (request) => {
      check(request);
      await openerCommands.revealItemInDir(path);
    },
    close: async () => {
      closed = true;
    },
  };
}

export async function openRemoteAttachment(
  connection: NativeRemoteConnection,
  vaultId: string,
  sessionId: string,
  attachmentId: string,
  options?: RequestOptions,
): Promise<AttachmentResource | null> {
  const info = await connection.request(
    {
      kind: "attachment_info",
      vault_id: vaultId,
      session_id: sessionId,
      attachment_id: attachmentId,
    },
    "attachment_info",
    options,
  );
  options?.signal?.throwIfAborted();
  if (!info) return null;
  const resource = await connection.openAttachment(
    vaultId,
    sessionId,
    attachmentId,
    options,
  );
  let closed = false;
  const copy = async (
    action: "open" | "reveal" | "save",
    request?: RequestOptions,
  ) => {
    if (closed) throw connectionClosed();
    return connection.copyAttachment(resource.id, action, request);
  };
  try {
    options?.signal?.throwIfAborted();
    return {
      id: resource.id,
      info,
      src:
        info.contentType.startsWith("image/") &&
        info.sizeBytes <= resource.preview_limit_bytes
          ? convertFileSrc(`attachments/${resource.id}`, "loofah-media")
          : undefined,
      labels: {
        open: msg`Open downloaded copy`,
        reveal: msg`Show downloaded copy`,
        save: msg`Save original copy`,
      },
      open: (request) => copy("open", request),
      reveal: (request) => copy("reveal", request),
      save: (request) => copy("save", request),
      close: async () => {
        if (closed) return;
        closed = true;
        await connection.closeAttachment(resource.id);
      },
    };
  } catch (error) {
    await connection.closeAttachment(resource.id).catch(() => {});
    throw error;
  }
}
