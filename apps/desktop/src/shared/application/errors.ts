import type { ApplicationError } from "./protocol.gen";

export class ApplicationRequestError extends Error {
  constructor(readonly detail: ApplicationError) {
    super(detail.message);
    this.name = "ApplicationRequestError";
  }
}

export function applicationResult<T>(
  result:
    | { status: "ok"; data: T }
    | { status: "error"; error: ApplicationError },
): T {
  if (result.status === "error")
    throw new ApplicationRequestError(result.error);
  return result.data;
}

export function connectionClosed(): ApplicationRequestError {
  return new ApplicationRequestError({
    code: "cancelled",
    message: "This server connection is closed",
    retryable: false,
    request_id: null,
    current_revision: null,
    mutation_outcome: "not_committed",
  });
}
