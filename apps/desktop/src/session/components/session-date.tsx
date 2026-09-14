import { useLingui } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { CheckIcon, XIcon } from "lucide-react";
import { useState } from "react";

import { Button } from "@hypr/ui/components/ui/button";
import { Input } from "@hypr/ui/components/ui/input";
import { sonnerToast } from "@hypr/ui/components/ui/toast";
import { format, safeFormat, safeParseDate } from "@hypr/utils";

import { useSession, useUpdateSession } from "~/session/queries";

export function SessionDate({ sessionId }: { sessionId: string }) {
  const { t } = useLingui();
  const [isEditing, setIsEditing] = useState(false);
  // Shown between closing the editor and the live query re-emitting, so the
  // read-only label never flashes the pre-save date. It masks the live value
  // until that value catches up (or the write fails), not until the write
  // resolves — the live query can lag the commit.
  const [pendingCreatedAt, setPendingCreatedAt] = useState<string | null>(null);
  const createdAt = useSession(sessionId)?.created_at;
  const effectiveCreatedAt =
    pendingCreatedAt !== null && createdAt !== pendingCreatedAt
      ? pendingCreatedAt
      : createdAt;
  const noteDate = safeFormat(
    effectiveCreatedAt ?? new Date(),
    "MMM d, yyyy h:mm a",
    t`Unknown date`,
  );

  if (!isEditing) {
    return (
      <button
        type="button"
        aria-label={t`Edit date`}
        title={t`Edit date`}
        onClick={() => setIsEditing(true)}
        className="text-muted-foreground hover:text-foreground flex h-6 w-fit items-center text-xs transition-none"
      >
        {noteDate}
      </button>
    );
  }

  return (
    <EditableDateForm
      key={`${createdAt ?? ""}`}
      sessionId={sessionId}
      createdAt={createdAt}
      onCancel={() => setIsEditing(false)}
      onSaved={(nextCreatedAt, commit) => {
        setIsEditing(false);
        setPendingCreatedAt(nextCreatedAt);
        void commit.catch((error) => {
          console.error("[session-date] failed to update session date", error);
          sonnerToast.error("Could not update the note date.");
          setPendingCreatedAt(null);
        });
      }}
    />
  );
}

function EditableDateForm({
  sessionId,
  createdAt,
  onCancel,
  onSaved,
}: {
  sessionId: string;
  createdAt: unknown;
  onCancel?: () => void;
  onSaved?: (nextCreatedAt: string, commit: Promise<unknown>) => void;
}) {
  const { t } = useLingui();
  const updateSession = useUpdateSession(sessionId);

  const form = useForm({
    defaultValues: {
      createdAt: toDatetimeLocalValue(createdAt),
    },
    validators: {
      onChange: ({ value }) => {
        if (!value.createdAt.trim()) {
          return {
            fields: {
              createdAt: t`Date and time are required`,
            },
          };
        }

        if (!toIsoString(value.createdAt)) {
          return {
            fields: {
              createdAt: t`Enter a valid date and time`,
            },
          };
        }

        return undefined;
      },
    },
    onSubmit: ({ value }) => {
      const nextCreatedAt = toIsoString(value.createdAt);
      if (!nextCreatedAt) {
        return;
      }

      onSaved?.(
        nextCreatedAt,
        Promise.resolve(updateSession({ created_at: nextCreatedAt })),
      );
    },
  });

  const commitOnBlur = () => {
    const value = form.state.values.createdAt;
    if (toIsoString(value) && value !== toDatetimeLocalValue(createdAt)) {
      void form.handleSubmit();
    } else {
      onCancel?.();
    }
  };

  return (
    <div className="flex w-fit flex-col gap-1">
      <form.Field name="createdAt">
        {(field) => (
          <div
            className="flex h-6 items-center gap-0"
            onBlur={(e) => {
              if (e.currentTarget.contains(e.relatedTarget)) {
                return;
              }
              commitOnBlur();
            }}
          >
            <Input
              autoFocus
              type="datetime-local"
              className="h-6 w-fit border-0 px-0 py-0 text-xs shadow-none focus-visible:ring-0"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  void form.handleSubmit();
                }

                if (e.key === "Escape") {
                  e.preventDefault();
                  onCancel?.();
                }
              }}
            />

            {onCancel && (
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="text-muted-foreground hover:bg-destructive/10 hover:text-destructive size-6 shrink-0 rounded-full"
                // Buttons don't take focus on click in WebKit, so without this
                // the input's blur would fire with relatedTarget null and
                // commit before the cancel click lands.
                onMouseDown={(e) => e.preventDefault()}
                onClick={onCancel}
                aria-label={t`Cancel date edit`}
              >
                <XIcon size={14} />
              </Button>
            )}

            <form.Subscribe selector={(state) => [state.canSubmit]}>
              {([canSubmit]) => (
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="text-muted-foreground hover:bg-brand/10 hover:text-brand size-6 shrink-0 rounded-full"
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => void form.handleSubmit()}
                  disabled={!canSubmit}
                  aria-label={t`Save date`}
                >
                  <CheckIcon size={14} />
                </Button>
              )}
            </form.Subscribe>
          </div>
        )}
      </form.Field>

      <form.Field name="createdAt">
        {(field) =>
          field.state.meta.errors[0] ? (
            <div className="text-destructive text-xs">
              {field.state.meta.errors[0]}
            </div>
          ) : null
        }
      </form.Field>
    </div>
  );
}

function toDatetimeLocalValue(value: unknown): string {
  const date = safeParseDate(value);
  if (!date) {
    return "";
  }

  return format(date, "yyyy-MM-dd'T'HH:mm");
}

function toIsoString(value: string): string | null {
  const parsed = safeParseDate(value);
  return parsed?.toISOString() ?? null;
}
