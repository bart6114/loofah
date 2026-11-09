import { useLingui } from "@lingui/react/macro";
import { useForm, useStore } from "@tanstack/react-form";
import { useMutation, useQuery } from "@tanstack/react-query";
import { CheckIcon, XIcon } from "lucide-react";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";

import {
  Popover,
  PopoverAnchor,
  PopoverContent,
} from "@hypr/ui/components/ui/popover";
import { sonnerToast } from "@hypr/ui/components/ui/toast";
import { cn } from "@hypr/utils";

import { useVaultHost } from "~/shared/application/vault-provider";
import type { VaultScope } from "~/shared/application/vault-scope";
import { normalizeTagNames } from "~/tags/normalize";
import { useTagSuggestions } from "~/tags/queries";

export function SessionTags({
  sessionId,
  className,
}: {
  sessionId: string;
  className?: string;
}) {
  const { scope } = useVaultHost();
  return scope ? (
    <TagConnection
      key={sessionId}
      scope={scope}
      sessionId={sessionId}
      className={className}
    />
  ) : null;
}

function TagConnection({
  scope,
  sessionId,
  className,
}: {
  scope: VaultScope;
  sessionId: string;
  className?: string;
}) {
  const connection = useSyncExternalStore(scope.subscribe, scope.getSnapshot);
  if (
    ["login_required", "access_denied", "incompatible", "closed"].includes(
      connection.state,
    )
  )
    return null;
  return (
    <TagEditor
      scope={scope}
      sessionId={sessionId}
      className={className}
      connected={connection.state === "connected"}
    />
  );
}

function TagEditor({
  scope,
  sessionId,
  className,
  connected,
}: {
  scope: VaultScope;
  sessionId: string;
  className?: string;
  connected: boolean;
}) {
  const { t } = useLingui();
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const metadata = useQuery(
    {
      ...scope.metadataOptions(sessionId),
      enabled: connected,
      notifyOnChangeProps: "all",
    },
    scope.queryClient,
  );
  const supported = scope.client.features.includes("tag_editing");
  const canWrite = () =>
    mounted.current &&
    supported &&
    scope.client.canEdit &&
    scope.getSnapshot().state === "connected";
  const mutation = useMutation(
    {
      networkMode: "always",
      retry: false,
      mutationFn: async (
        input:
          | {
              kind: "replace";
              tags: string[];
              name: string | null;
              revision: string;
            }
          | { kind: "accept" | "dismiss"; name: string; revision: string },
      ) => {
        if (!canWrite()) throw new Error(t`Could not update tags.`);
        const outcome = await scope.run(() =>
          input.kind === "replace"
            ? scope.client.saveMetadata(sessionId, {
                patch: { tags: input.tags },
                expected_revision: input.revision,
              })
            : input.kind === "accept"
              ? scope.client.acceptTagSuggestion(sessionId, {
                  name: input.name,
                  expected_revision: input.revision,
                })
              : scope.client.dismissTagSuggestion(sessionId, {
                  name: input.name,
                  expected_revision: input.revision,
                }),
        );
        if (
          outcome.kind === "saved" &&
          input.kind === "replace" &&
          input.name &&
          canWrite()
        ) {
          const name = input.name;
          await scope.run(() => scope.client.ensureTag({ name }));
        }
        return {
          outcome,
          readCount:
            scope.queryClient.getQueryState(
              scope.metadataOptions(sessionId).queryKey,
            )?.dataUpdateCount ?? 0,
        };
      },
      onSuccess: async () => {
        if (scope.getSnapshot().state !== "connected") return;
        // A write reply can predate a newer event; read canonical metadata instead of re-admitting it.
        await scope.queryClient.invalidateQueries({
          queryKey: scope.metadataOptions(sessionId).queryKey,
          exact: true,
        });
        if (scope.getSnapshot().state !== "connected") return;
        for (const kind of ["tags", "tag_suggestions"])
          void scope.queryClient.invalidateQueries({
            queryKey: ["vault", scope.client.scope, kind],
          });
      },
      onError: () => {
        if (scope.getSnapshot().state !== "connected") return;
        void scope.queryClient.invalidateQueries({
          queryKey: scope.metadataOptions(sessionId).queryKey,
          exact: true,
          refetchType: "none",
        });
        if (mounted.current) sonnerToast.error(t`Could not update tags.`);
      },
    },
    scope.queryClient,
  );
  const savedTags = metadata.data?.meta.tags ?? EMPTY_TAGS;
  // Content hashes can return to an earlier revision; confirm a read, not a different hash.
  const awaitingCanonical =
    mutation.data?.outcome.kind === "saved" &&
    (scope.queryClient.getQueryState(scope.metadataOptions(sessionId).queryKey)
      ?.dataUpdateCount ?? 0) <= mutation.data.readCount;
  const tags =
    mutation.variables?.kind === "replace" &&
    (mutation.isPending ||
      (awaitingCanonical &&
        metadata.data?.revision === mutation.variables.revision))
      ? mutation.variables.tags
      : savedTags;
  const review = Boolean(
    mutation.error ||
    metadata.error ||
    mutation.data?.outcome.kind === "conflict",
  );
  const disabled =
    !connected ||
    !scope.client.canEdit ||
    !supported ||
    !metadata.data ||
    mutation.isPending ||
    awaitingCanonical ||
    review;
  const commit = (nextTags: string[], name: string | null = null) => {
    if (disabled || !canWrite() || !metadata.data) return;
    const sorted = [...new Set(nextTags)].sort();
    if (tagsEqual(sorted, tags)) return;
    mutation.mutate({
      kind: "replace",
      tags: sorted,
      name,
      revision: metadata.data.revision,
    });
  };
  const addTag = (name: string) => {
    const normalized = normalizeTagNames([name])[0];
    if (normalized && !tags.includes(normalized))
      commit([...tags, normalized], normalized);
  };
  const suggestedTags =
    metadata.data?.meta.tag_suggestions?.items.filter(
      (name) => !tags.includes(name),
    ) ?? EMPTY_TAGS;
  const failure = mutation.error ?? metadata.error;
  return (
    <div className={cn(["flex flex-wrap items-center gap-1.5", className])}>
      {tags.map((tag) => (
        <span
          key={tag}
          className={cn([
            "group/tag flex items-center",
            "bg-accent/60 text-muted-foreground rounded-full px-2 py-0.5 text-xs font-medium",
          ])}
        >
          #{tag}
          <button
            type="button"
            disabled={disabled}
            aria-label={t`Remove tag ${tag}`}
            onClick={() => commit(tags.filter((other) => other !== tag))}
            className={cn([
              "ml-0 w-0 shrink-0 overflow-hidden rounded-full opacity-0",
              "transition-[width,margin-left,opacity] duration-150 motion-reduce:transition-none",
              "group-hover/tag:ml-1 group-hover/tag:w-3 group-hover/tag:opacity-60",
              "group-focus-within/tag:ml-1 group-focus-within/tag:w-3 group-focus-within/tag:opacity-60",
              "hover:!opacity-100 focus-visible:opacity-100 disabled:opacity-30",
            ])}
          >
            <XIcon size={12} />
          </button>
        </span>
      ))}
      {suggestedTags.map((name) => (
        <span
          key={name}
          className={cn([
            "flex items-center gap-1 rounded-full border border-dashed",
            "border-muted-foreground/40 text-muted-foreground py-0.5 pr-1 pl-2 text-xs font-medium",
          ])}
        >
          #{name}
          <button
            type="button"
            disabled={disabled}
            aria-label={t`Accept suggested tag ${name}`}
            onClick={() =>
              !disabled &&
              metadata.data &&
              canWrite() &&
              mutation.mutate({
                kind: "accept",
                name,
                revision: metadata.data.revision,
              })
            }
            className="hover:text-foreground rounded-full opacity-70 disabled:opacity-30"
          >
            <CheckIcon size={12} />
          </button>
          <button
            type="button"
            disabled={disabled}
            aria-label={t`Dismiss suggested tag ${name}`}
            onClick={() =>
              !disabled &&
              metadata.data &&
              canWrite() &&
              mutation.mutate({
                kind: "dismiss",
                name,
                revision: metadata.data.revision,
              })
            }
            className="hover:text-foreground rounded-full opacity-70 disabled:opacity-30"
          >
            <XIcon size={12} />
          </button>
        </span>
      ))}
      <TagAddControl
        scope={scope}
        attachedTags={tags}
        onAdd={addTag}
        disabled={disabled}
      />
      {review && (
        <span role="alert" className="text-destructive text-xs">
          {failure?.message ?? t`Could not update tags.`}
          <button
            type="button"
            disabled={!connected || metadata.isFetching || mutation.isPending}
            onClick={() => {
              if (scope.getSnapshot().state !== "connected") return;
              void metadata
                .refetch()
                .then((result) => {
                  if (
                    mounted.current &&
                    !result.error &&
                    !result.isFetching &&
                    !result.isPaused &&
                    result.data &&
                    scope.getSnapshot().state === "connected"
                  )
                    mutation.reset();
                })
                .catch(() => {});
            }}
          >{t`Reload tags`}</button>
        </span>
      )}
    </div>
  );
}

function TagAddControl({
  scope,
  attachedTags,
  onAdd,
  disabled,
}: {
  scope: VaultScope;
  attachedTags: string[];
  onAdd: (name: string) => void;
  disabled: boolean;
}) {
  const { t } = useLingui();
  const form = useForm({ defaultValues: { name: "" } });
  const draft = useStore(form.store, (state) => state.values.name);
  const [editing, setEditing] = useState(false);
  const [query, setQuery] = useState("");
  const [highlightIndex, setHighlightIndex] = useState(-1);
  const committed = useRef(false);
  useEffect(() => {
    const timer = setTimeout(() => setQuery(draft), 120);
    return () => clearTimeout(timer);
  }, [draft]);
  const lookup = useTagSuggestions(
    scope,
    { query, excluded: attachedTags, limit: 8 },
    editing && !disabled && query === draft,
  );
  const suggestions =
    editing && query === draft ? (lookup.data ?? EMPTY_TAGS) : EMPTY_TAGS;
  useEffect(() => setHighlightIndex(-1), [draft, lookup.data]);
  const commitDraft = (name: string) => {
    if (committed.current || disabled) return;
    committed.current = true;
    setEditing(false);
    if (name.trim()) onAdd(name);
  };
  if (!editing)
    return (
      <button
        type="button"
        disabled={disabled}
        onClick={() => {
          committed.current = false;
          form.reset();
          setQuery("");
          setHighlightIndex(-1);
          setEditing(true);
        }}
        className={cn([
          "text-muted-foreground hover:text-foreground rounded-full py-0.5 text-xs transition-none disabled:opacity-30",
          attachedTags.length === 0 ? "pr-2" : "px-1",
        ])}
      >
        {attachedTags.length === 0 ? t`+ Add tag` : "+"}
      </button>
    );
  return (
    <>
      <Popover open={suggestions.length > 0 && !disabled}>
        <PopoverAnchor asChild>
          <input
            autoFocus
            type="text"
            disabled={disabled}
            value={draft}
            placeholder={t`Tag name`}
            onChange={(event) => form.setFieldValue("name", event.target.value)}
            onBlur={() => commitDraft(draft)}
            onKeyDown={(event) => {
              if (event.key === "ArrowDown") {
                event.preventDefault();
                setHighlightIndex((index) =>
                  suggestions.length === 0
                    ? -1
                    : (index + 1) % suggestions.length,
                );
              } else if (event.key === "ArrowUp") {
                event.preventDefault();
                setHighlightIndex((index) =>
                  suggestions.length === 0
                    ? -1
                    : (index <= 0 ? suggestions.length : index) - 1,
                );
              } else if (event.key === "Enter") {
                event.preventDefault();
                commitDraft(suggestions[highlightIndex] ?? draft);
              } else if (event.key === "Escape") {
                event.preventDefault();
                committed.current = true;
                setEditing(false);
              }
            }}
            className={cn([
              "text-muted-foreground w-24 min-w-0 rounded-full py-0.5 text-xs",
              "border-none bg-transparent outline-hidden",
            ])}
          />
        </PopoverAnchor>
        <PopoverContent
          align="start"
          className="w-48 p-1"
          onOpenAutoFocus={(event) => event.preventDefault()}
        >
          <ul role="listbox">
            {suggestions.map((name, index) => (
              <li key={name}>
                <button
                  type="button"
                  role="option"
                  aria-selected={index === highlightIndex}
                  onPointerDown={(event) => event.preventDefault()}
                  onClick={() => commitDraft(name)}
                  onMouseEnter={() => setHighlightIndex(index)}
                  className={cn([
                    "w-full rounded-sm px-2 py-1 text-left text-sm",
                    index === highlightIndex && "bg-accent",
                  ])}
                >
                  #{name}
                </button>
              </li>
            ))}
          </ul>
        </PopoverContent>
      </Popover>
      {lookup.error && (
        <span role="alert" className="text-destructive text-xs">
          {lookup.error.message}
        </span>
      )}
    </>
  );
}

function tagsEqual(a: string[], b: string[]) {
  return a.length === b.length && a.every((tag, index) => tag === b[index]);
}
const EMPTY_TAGS: string[] = [];
