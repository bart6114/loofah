import { Trans, useLingui } from "@lingui/react/macro";
import { useForm, useStore } from "@tanstack/react-form";
import { onlineManager, useMutation, useQuery } from "@tanstack/react-query";
import { usePrevious } from "@uidotdev/usehooks";
import {
  type CSSProperties,
  forwardRef,
  memo,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { useResizeObserver } from "usehooks-ts";

import { cn } from "@hypr/utils";

import { useTitleGenerating } from "~/ai/hooks";
import {
  ApplicationRequestError,
  connectionClosed,
} from "~/shared/application/errors";
import { captureSessionTitleGuard } from "~/shared/application/session-title-guard";
import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";
import type { VaultScope } from "~/shared/application/vault-scope";
import {
  liveSessionTitleDraft,
  useLiveSessionTitleDraft,
  useLiveTitle,
  type LiveTitleDraft,
} from "~/store/zustand/live-title";
import { type Tab } from "~/store/zustand/tabs";

export interface TitleInputHandle {
  focus: () => void;
  focusAtEnd: () => void;
  focusAtPixelWidth: (pixelWidth: number) => void;
}

export const TitleInput = forwardRef<
  TitleInputHandle,
  {
    tab: Extract<Tab, { type: "sessions" }>;
    onTransferContentToEditor?: (content: string) => void;
    onFocusEditorAtStart?: () => void;
    onFocusEditorAtPixelWidth?: (pixelWidth: number) => void;
    variant?: "title" | "breadcrumb";
  }
>(
  (
    {
      tab,
      onTransferContentToEditor,
      onFocusEditorAtStart,
      onFocusEditorAtPixelWidth,
      variant = "title",
    },
    ref,
  ) => {
    const {
      id: sessionId,
      state: { view },
    } = tab;
    const owner = useVaultReadScope();
    const local = owner.scope.client.features.includes(
      "legacy_desktop_enhance",
    );
    if (!owner.current()) return null;
    const props = {
      sessionId,
      editorId: view ? "active" : "inactive",
      onTransferContentToEditor,
      onFocusEditorAtStart,
      onFocusEditorAtPixelWidth,
      variant,
      scope: owner.scope,
      current: owner.current,
    };
    return local ? (
      <LegacyTitleInput
        key={`${sessionId}:${tab.slotId}`}
        {...props}
        ref={ref}
      />
    ) : (
      <TitleInputInner
        key={`${sessionId}:${tab.slotId}`}
        {...props}
        ref={ref}
        isGenerating={false}
      />
    );
  },
);

type TitleFieldsProps = {
  scope: VaultScope;
  current: () => boolean;
  sessionId: string;
  editorId: string;
  onTransferContentToEditor?: (content: string) => void;
  onFocusEditorAtStart?: () => void;
  onFocusEditorAtPixelWidth?: (pixelWidth: number) => void;
  variant: "title" | "breadcrumb";
};
const LegacyTitleInput = forwardRef<TitleInputHandle, TitleFieldsProps>(
  (props, ref) => {
    const isGenerating = useTitleGenerating(props.sessionId);
    return <TitleInputInner {...props} ref={ref} isGenerating={isGenerating} />;
  },
);

const TitleInputInner = memo(
  forwardRef<TitleInputHandle, TitleFieldsProps & { isGenerating: boolean }>(
    (
      {
        scope,
        current,
        isGenerating,
        sessionId,
        editorId,
        onTransferContentToEditor,
        onFocusEditorAtStart,
        onFocusEditorAtPixelWidth,
        variant,
      },
      ref,
    ) => {
      const { t } = useLingui();
      const mounted = useRef(false);
      const token = useRef(
        liveSessionTitleDraft(scope, sessionId)?.owner ?? {},
      ).current;
      const initial = useRef(liveSessionTitleDraft(scope, sessionId)).current;
      useLayoutEffect(() => {
        mounted.current = true;
        return () => {
          mounted.current = false;
        };
      }, []);
      const online = useSyncExternalStore(
        (changed) => onlineManager.subscribe(changed),
        () => onlineManager.isOnline(),
      );
      const active = () => mounted.current && current();
      const available = () =>
        active() &&
        scope.getSnapshot().state === "connected" &&
        scope.client.canEdit &&
        scope.client.features.includes("metadata_edit") &&
        (scope.client.networkMode === "always" || onlineManager.isOnline());
      const options = scope.metadataOptions(sessionId);
      const metadata = useQuery(
        {
          ...options,
          enabled:
            !!sessionId &&
            current() &&
            scope.client.features.includes("content_read"),
          notifyOnChangeProps: "all",
        },
        scope.queryClient,
      );
      const storeTitle =
        metadata.data?.meta.id === sessionId
          ? metadata.data.meta.title
          : undefined;
      const readFresh = async () => {
        if (!available()) throw connectionClosed();
        await scope.queryClient.cancelQueries({
          queryKey: options.queryKey,
          exact: true,
        });
        if (!available()) throw connectionClosed();
        const value = await scope.queryClient.fetchQuery({
          ...options,
          staleTime: 0,
          retry: false,
        });
        if (!available() || value.meta.id !== sessionId)
          throw connectionClosed();
        return scope.queryClient.getQueryData(options.queryKey) ?? value;
      };
      const write = useMutation(
        {
          networkMode: "always",
          retry: false,
          mutationKey: ["vault", scope.client.scope, "title_write", sessionId],
          mutationFn: async (draft: LiveTitleDraft) => {
            if (!available()) throw connectionClosed();
            const guard = captureSessionTitleGuard(scope, sessionId, active);
            let sent = false;
            try {
              await guard.prepare();
              guard.assertReady();
              if (!available()) throw connectionClosed();
              sent = true;
              const outcome = await scope.run(() =>
                scope.client.saveMetadata(sessionId, {
                  patch: { title: draft.title },
                  expected_revision: draft.revision,
                }),
              );
              if (!available()) throw connectionClosed();
              for (const kind of [
                "session",
                "note",
                "note_editor",
                "document_editor",
                "docs",
              ])
                void scope.queryClient.invalidateQueries({
                  queryKey: ["vault", scope.client.scope, kind, sessionId],
                });
              for (const kind of ["session_summaries", "session_page"])
                void scope.queryClient.invalidateQueries({
                  queryKey: ["vault", scope.client.scope, kind],
                });
              const canonical = await readFresh().catch(() => undefined);
              if (!available()) throw connectionClosed();
              return { outcome, canonical };
            } catch (error) {
              const uncertain =
                sent &&
                (!(error instanceof ApplicationRequestError) ||
                  error.detail.mutation_outcome !== "not_committed");
              const latest = liveSessionTitleDraft(scope, sessionId);
              if (uncertain && latest?.owner === draft.owner)
                useLiveTitle
                  .getState()
                  .replaceTitle(latest, { ...latest, uncertain: true });
              throw error;
            } finally {
              guard.dispose();
            }
          },
          onSuccess: ({ outcome, canonical }, draft) => {
            if (!active()) return;
            const latest = liveSessionTitleDraft(scope, sessionId);
            if (outcome.kind !== "saved" || !canonical) {
              if (latest?.owner === draft.owner)
                useLiveTitle
                  .getState()
                  .replaceTitle(latest, { ...latest, uncertain: true });
              return;
            }
            if (canonical.meta.title !== draft.title || latest !== draft)
              return;
            useLiveTitle.getState().clearTitle(draft);
            form.reset({
              title: canonical.meta.title,
              revision: canonical.revision,
              baselineTitle: canonical.meta.title,
            });
          },
        },
        scope.queryClient,
      );
      const review = useMutation(
        { networkMode: "always", retry: false, mutationFn: readFresh },
        scope.queryClient,
      );
      const form = useForm({
        defaultValues: {
          title: initial?.title ?? "",
          revision: initial?.revision ?? "",
          baselineTitle: initial?.baselineTitle ?? "",
        },
        onSubmit: async ({ value }) => {
          if (
            !available() ||
            write.isPending ||
            review.isPending ||
            needsReview
          )
            return;
          const draft = liveSessionTitleDraft(scope, sessionId);
          if (
            !draft ||
            draft.owner !== token ||
            draft.title !== value.title ||
            draft.title === draft.baselineTitle
          )
            return;
          await write.mutateAsync(draft).catch(() => {});
        },
      });
      const values = useStore(form.store, (state) => state.values);
      const submitting = useStore(form.store, (state) => state.isSubmitting);
      const entry = useLiveSessionTitleDraft(scope, sessionId);
      const ownDraft = entry?.owner === token ? entry : undefined;
      const pendingWrite = useRef(false);
      pendingWrite.current = write.isPending || submitting;
      useEffect(
        () => () => {
          const draft = liveSessionTitleDraft(scope, sessionId);
          const saved = scope.queryClient.getQueryData(
            scope.metadataOptions(sessionId).queryKey,
          );
          if (
            draft?.owner === token &&
            draft.title === draft.baselineTitle &&
            saved?.revision === draft.revision &&
            saved.meta.title === draft.title &&
            !draft.uncertain &&
            !pendingWrite.current
          )
            useLiveTitle.getState().clearTitle(draft);
        },
        [scope, sessionId, token],
      );
      const stale =
        !!ownDraft &&
        !!metadata.data &&
        values.revision !== metadata.data.revision;
      const needsReview =
        !!write.error ||
        !!review.error ||
        write.data?.outcome.kind === "conflict" ||
        !!ownDraft?.uncertain ||
        stale ||
        (!!write.data && !write.data.canonical);
      const disabled =
        !current() ||
        scope.getSnapshot().state !== "connected" ||
        !scope.client.canEdit ||
        !scope.client.features.includes("metadata_edit") ||
        (!online && scope.client.networkMode !== "always") ||
        storeTitle === undefined;
      const [isOverflowing, setIsOverflowing] = useState(false);
      const [overflowDistance, setOverflowDistance] = useState(0);
      const [showStartFade, setShowStartFade] = useState(false);
      const [showEndFade, setShowEndFade] = useState(false);
      const [isTitleFocused, setIsTitleFocused] = useState(false);
      const internalRef = useRef<HTMLInputElement>(null);
      const wasGenerating = usePrevious(isGenerating);
      const [showRevealAnimation, setShowRevealAnimation] = useState(false);
      useEffect(() => {
        if (wasGenerating && !isGenerating && !ownDraft) {
          setShowRevealAnimation(true);
          const timer = setTimeout(() => setShowRevealAnimation(false), 1000);
          return () => clearTimeout(timer);
        }
      }, [wasGenerating, isGenerating, ownDraft]);
      const title = ownDraft ? values.title : (storeTitle ?? "");
      const capture = (value: string) => {
        if (!available() || !metadata.data) return;
        if (!ownDraft) {
          form.setFieldValue("revision", metadata.data.revision);
          form.setFieldValue("baselineTitle", metadata.data.meta.title);
        }
        form.setFieldValue("title", value);
        const baseline = form.state.values;
        useLiveTitle.getState().setTitle({
          scope,
          sessionId,
          owner: token,
          title: value,
          revision: baseline.revision,
          baselineTitle: baseline.baselineTitle,
          uncertain: ownDraft?.uncertain ?? false,
          legacy: scope.client.features.includes("legacy_desktop_enhance"),
        });
      };
      const submit = () => {
        if (
          available() &&
          !write.isPending &&
          !review.isPending &&
          !form.state.isSubmitting &&
          !needsReview
        )
          void form.handleSubmit().catch(() => {});
      };
      const reviewSaved = () => {
        if (!available() || write.isPending || review.isPending) return;
        const draft = liveSessionTitleDraft(scope, sessionId);
        void review
          .mutateAsync()
          .then((snapshot) => {
            if (
              !available() ||
              liveSessionTitleDraft(scope, sessionId) !== draft ||
              !draft ||
              draft.owner !== token
            )
              return;
            form.setFieldValue("revision", snapshot.revision);
            form.setFieldValue("baselineTitle", snapshot.meta.title);
            write.reset();
            review.reset();
            if (form.state.values.title === snapshot.meta.title) {
              useLiveTitle.getState().clearTitle(draft);
              form.reset({
                title: snapshot.meta.title,
                revision: snapshot.revision,
                baselineTitle: snapshot.meta.title,
              });
            } else
              useLiveTitle.getState().replaceTitle(draft, {
                ...draft,
                revision: snapshot.revision,
                baselineTitle: snapshot.meta.title,
                uncertain: false,
              });
          })
          .catch(() => {});
      };

      const updateOverflowState = useCallback(
        (node?: HTMLInputElement | null) => {
          const input = node ?? internalRef.current;
          if (!input) {
            setIsOverflowing(false);
            setOverflowDistance(0);
            setShowStartFade(false);
            setShowEndFade(false);
            return;
          }
          const distance = Math.max(input.scrollWidth - input.clientWidth, 0);
          const overflowing = distance > 1;
          const scrollLeft = Math.max(input.scrollLeft, 0);
          setIsOverflowing(distance > 1);
          setOverflowDistance(distance);
          setShowStartFade(overflowing && scrollLeft > 1);
          setShowEndFade(overflowing && scrollLeft < distance - 1);
        },
        [],
      );

      const setInputRef = useCallback(
        (node: HTMLInputElement | null) => {
          internalRef.current = node;
          if (node) {
            requestAnimationFrame(() => updateOverflowState(node));
          } else {
            setIsOverflowing(false);
            setOverflowDistance(0);
            setShowStartFade(false);
            setShowEndFade(false);
          }
        },
        [updateOverflowState],
      );

      useResizeObserver({
        ref: internalRef as React.RefObject<HTMLInputElement>,
        onResize: () => updateOverflowState(),
      });

      const titleFadeStyle =
        showStartFade || showEndFade
          ? {
              WebkitMaskImage: getTitleFadeMask({
                showStartFade,
                showEndFade,
              }),
              maskImage: getTitleFadeMask({ showStartFade, showEndFade }),
              WebkitMaskRepeat: "no-repeat",
              maskRepeat: "no-repeat",
              WebkitMaskSize: "100% 100%",
              maskSize: "100% 100%",
            }
          : undefined;
      const showHoverReveal =
        isOverflowing && !isTitleFocused && title.length > 0;
      const titleHoverScrollStyle = showHoverReveal
        ? ({
            "--title-hover-scroll-distance": `-${Math.ceil(
              overflowDistance,
            )}px`,
            "--title-hover-scroll-duration": `${Math.min(
              Math.max(overflowDistance / 48, 2.5),
              8,
            ).toFixed(2)}s`,
          } as CSSProperties)
        : undefined;
      const visibleTitleLength = Math.max(
        title.length || t`Untitled`.length,
        t`Untitled`.length,
      );
      const titleShellStyle = {
        ...titleFadeStyle,
        width: `calc(${visibleTitleLength}ch + 2px)`,
      };

      useImperativeHandle(
        ref,
        () => ({
          focus: () => internalRef.current?.focus(),
          focusAtEnd: () => {
            const input = internalRef.current;
            if (input) {
              input.focus();
              input.setSelectionRange(input.value.length, input.value.length);
            }
          },
          focusAtPixelWidth: (pixelWidth: number) => {
            const input = internalRef.current;
            if (input && input.value) {
              input.focus();
              const titleStyle = window.getComputedStyle(input);
              const canvas = document.createElement("canvas");
              const ctx = canvas.getContext("2d");
              if (ctx) {
                ctx.font = `${titleStyle.fontWeight} ${titleStyle.fontSize} ${titleStyle.fontFamily}`;
                let charPos = 0;
                for (let i = 0; i <= input.value.length; i++) {
                  const currentWidth = ctx.measureText(
                    input.value.slice(0, i),
                  ).width;
                  if (currentWidth >= pixelWidth) {
                    charPos = i;
                    break;
                  }
                  charPos = i;
                }
                input.setSelectionRange(charPos, charPos);
              }
            } else if (input) {
              input.focus();
            }
          },
        }),
        [],
      );

      useLayoutEffect(() => {
        requestAnimationFrame(() => updateOverflowState());
      }, [title, updateOverflowState]);

      const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
        if (disabled || !active()) return;
        if (isComposingKeyEvent(e)) {
          return;
        }

        if (e.key === "ArrowUp") {
          e.preventDefault();
          return;
        }

        if (e.key === "Enter") {
          e.preventDefault();
          const input = internalRef.current;
          if (!input) return;

          if (!onTransferContentToEditor && !onFocusEditorAtStart) {
            input.blur();
            return;
          }

          const cursorPos = input.selectionStart ?? input.value.length;
          const beforeCursor = input.value.slice(0, cursorPos);
          const afterCursor = input.value.slice(cursorPos);

          capture(beforeCursor);
          submit();

          if (afterCursor) {
            setTimeout(() => {
              if (available()) onTransferContentToEditor?.(afterCursor);
            }, 0);
          } else {
            setTimeout(() => {
              if (available()) onFocusEditorAtStart?.();
            }, 0);
          }
        } else if (e.key === "Tab") {
          e.preventDefault();
          setTimeout(() => {
            if (available()) onFocusEditorAtStart?.();
          }, 0);
        } else if (e.key === "ArrowRight") {
          const input = internalRef.current;
          if (!input) return;
          const cursorPos = input.selectionStart ?? 0;
          if (
            cursorPos === input.value.length &&
            input.selectionEnd === cursorPos
          ) {
            e.preventDefault();
            setTimeout(() => {
              if (available()) onFocusEditorAtStart?.();
            }, 0);
          }
        } else if (e.key === "ArrowDown") {
          e.preventDefault();
          const input = internalRef.current;
          if (!input) return;

          const cursorPos = input.selectionStart ?? 0;
          const textBeforeCursor = input.value.slice(0, cursorPos);

          const canvas = document.createElement("canvas");
          const ctx = canvas.getContext("2d");
          if (ctx) {
            const titleStyle = window.getComputedStyle(input);
            ctx.font = `${titleStyle.fontWeight} ${titleStyle.fontSize} ${titleStyle.fontFamily}`;
            const titleWidth = ctx.measureText(textBeforeCursor).width;
            setTimeout(() => {
              if (available()) onFocusEditorAtPixelWidth?.(titleWidth);
            }, 0);
          }
        }
      };

      if (!current()) return null;
      return (
        <div className="flex max-w-full flex-col gap-1">
          <div
            data-tauri-drag-region="false"
            style={titleShellStyle}
            className={cn([
              "group/title-input relative flex max-w-full items-center overflow-hidden",
              variant === "breadcrumb"
                ? "h-5 text-sm leading-5"
                : "session-title-heading h-[34px] text-[26px] leading-[34px] font-semibold tracking-[-0.025em]",
            ])}
          >
            <input
              data-tauri-drag-region="false"
              data-session-title-input
              aria-label={t`Session title`}
              ref={setInputRef}
              id={`title-input-${sessionId}-${editorId}`}
              placeholder={t`Untitled`}
              type="text"
              readOnly={disabled}
              aria-busy={write.isPending || submitting}
              onChange={(e) => {
                const value = e.target.value;
                capture(value);
                updateOverflowState(e.target);
              }}
              onClick={(e) => updateOverflowState(e.currentTarget)}
              onKeyDown={handleKeyDown}
              onKeyUp={(e) => updateOverflowState(e.currentTarget)}
              onFocus={() => {
                capture(title);
                setIsTitleFocused(true);
                updateOverflowState();
              }}
              onBlur={(e) => {
                setIsTitleFocused(false);
                const draft = liveSessionTitleDraft(scope, sessionId);
                if (
                  draft?.owner === token &&
                  draft.title === draft.baselineTitle &&
                  !draft.uncertain &&
                  !needsReview &&
                  !write.isPending
                )
                  useLiveTitle.getState().clearTitle(draft);
                else submit();
                updateOverflowState(e.target);
              }}
              onScroll={(e) => updateOverflowState(e.currentTarget)}
              onSelect={(e) => updateOverflowState(e.currentTarget)}
              value={title}
              size={visibleTitleLength}
              className={cn([
                "w-full min-w-0 transition-opacity duration-200",
                "border-none bg-transparent focus:outline-hidden",
                "placeholder:text-muted-foreground text-left",
                variant === "breadcrumb"
                  ? "text-muted-foreground h-5 appearance-none p-0 text-sm leading-5 focus:underline"
                  : "text-[26px] leading-[34px] font-semibold tracking-[-0.025em]",
                variant === "breadcrumb" &&
                  (isTitleFocused
                    ? "overflow-x-auto whitespace-nowrap"
                    : "truncate"),
                showHoverReveal && "text-transparent caret-transparent",
                isGenerating && !ownDraft && "hidden",
                showRevealAnimation && !ownDraft && "animate-reveal-left",
              ])}
            />
            {isGenerating && !ownDraft ? (
              <span className="text-muted-foreground animate-pulse">
                <Trans>Generating title...</Trans>
              </span>
            ) : null}
            {showHoverReveal ? (
              <div
                aria-hidden="true"
                className="pointer-events-none absolute inset-0 flex items-center justify-start overflow-hidden"
              >
                <span
                  style={titleHoverScrollStyle}
                  className={cn([
                    "group-hover/title-input:animate-title-hover-scroll whitespace-nowrap group-hover/title-input:will-change-transform",
                    variant === "breadcrumb"
                      ? "text-sm leading-5"
                      : "text-[26px] leading-[34px] font-semibold tracking-[-0.025em]",
                  ])}
                >
                  {title}
                </span>
              </div>
            ) : null}
          </div>
          {needsReview ? (
            <div role="alert" className="text-muted-foreground text-xs">
              <p>
                {write.error?.message ??
                  review.error?.message ??
                  t`This title changed or its save could not be confirmed. Your draft is kept.`}
              </p>
              <p>
                <Trans>Saved title: {storeTitle ?? ""}</Trans>
              </p>
              <button
                type="button"
                disabled={disabled || write.isPending || review.isPending}
                onClick={reviewSaved}
              >
                <Trans>Review saved title</Trans>
              </button>
            </div>
          ) : ownDraft &&
            ownDraft.title !== ownDraft.baselineTitle &&
            !isTitleFocused ? (
            <button
              type="button"
              disabled={disabled || write.isPending || review.isPending}
              onClick={submit}
            >
              <Trans>Save title</Trans>
            </button>
          ) : null}
        </div>
      );
    },
  ),
);

function isComposingKeyEvent(event: React.KeyboardEvent<HTMLInputElement>) {
  return (
    event.nativeEvent.isComposing ||
    event.key === "Process" ||
    event.keyCode === 229
  );
}

function getTitleFadeMask({
  showStartFade,
  showEndFade,
}: {
  showStartFade: boolean;
  showEndFade: boolean;
}) {
  if (showStartFade && showEndFade) {
    return "linear-gradient(to right, transparent 0, black 28px, black calc(100% - 28px), transparent 100%)";
  }

  if (showStartFade) {
    return "linear-gradient(to right, transparent 0, black 28px, black 100%)";
  }

  return "linear-gradient(to right, black 0, black calc(100% - 28px), transparent 100%)";
}
