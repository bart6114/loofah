import { Trans, useLingui } from "@lingui/react/macro";
import { useForm } from "@tanstack/react-form";
import { useIsMutating, useMutation, useQuery } from "@tanstack/react-query";
import { Info } from "lucide-react";
import { useEffect, useRef } from "react";

import { commands as localSttCommands } from "@hypr/plugin-local-stt";
import { Button } from "@hypr/ui/components/ui/button";
import { Input } from "@hypr/ui/components/ui/input";
import { Kbd } from "@hypr/ui/components/ui/kbd";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@hypr/ui/components/ui/select";
import { Switch } from "@hypr/ui/components/ui/switch";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@hypr/ui/components/ui/tooltip";

import {
  configureDictationShortcut,
  isDictationActive,
  useDictationShortcut,
  useDictationStatus,
} from "~/dictation/state";
import { type StoredSettingValues, setSettingValues } from "~/settings/queries";
import { resolveConfigValues, useConfigValues } from "~/shared/config";
import { usePermission } from "~/shared/hooks/usePermissions";
import { useTabs } from "~/store/zustand/tabs";
import { isSupportedLocalSttModel } from "~/stt/capabilities";

export function parseDictationShortcut(
  event: {
    key: string;
    ctrlKey: boolean;
    altKey: boolean;
    shiftKey: boolean;
    metaKey: boolean;
  },
  isMac: boolean,
): string | null {
  const key = event.key === " " ? "Space" : event.key.toUpperCase();
  if (!/^(Space|[A-Z0-9])$/.test(key)) return null;
  const modifiers = [
    event.ctrlKey && "Ctrl",
    event.altKey && (isMac ? "Option" : "Alt"),
    event.shiftKey && "Shift",
    event.metaKey && (isMac ? "Cmd" : "Win"),
  ].filter(Boolean);
  return modifiers.length >= 2 ? [...modifiers, key].join("+") : null;
}

const shortcutMutationKey = ["dictation-shortcut-edit"];

export function DictationSettings({
  storedSettings,
}: {
  storedSettings: StoredSettingValues;
}) {
  const { t } = useLingui();
  const changeButton = useRef<HTMLButtonElement>(null);
  const shortcutEdit = useRef<{ cancelled: boolean } | null>(null);
  const mounted = useRef(true);
  const shortcutPending =
    useIsMutating({ mutationKey: shortcutMutationKey }) > 0;
  const values = resolveConfigValues(
    [
      "dictation_enabled",
      "dictation_shortcut",
      "dictation_trigger_mode",
    ] as const,
    storedSettings,
  );
  const { data: status } = useDictationStatus();
  const busy = isDictationActive(status);
  const capturing = useDictationShortcut((state) => state.capturing);
  const save = useMutation({
    mutationFn: (value: typeof values) => setSettingValues(value),
    onError: () => form.reset(values),
  });
  const form = useForm({
    defaultValues: values,
    onSubmit: ({ value }) => save.mutate(value),
  });
  const capture = useMutation({
    mutationFn: async () => {
      useDictationShortcut.setState({ capturing: true });
      await configureDictationShortcut("");
    },
    onError: () => useDictationShortcut.setState({ capturing: false }),
  });
  const shortcut = useMutation({
    mutationKey: shortcutMutationKey,
    mutationFn: async (event: Parameters<typeof parseDictationShortcut>[0]) => {
      const value = parseDictationShortcut(
        event,
        navigator.platform.includes("Mac"),
      );
      if (!value)
        throw new Error(
          navigator.platform.includes("Mac")
            ? t`Use two modifier keys and a letter, number, or Space. With Option, try Space.`
            : t`Press two modifier keys with a letter, number, or Space. Escape cancels.`,
        );
      if (shortcutEdit.current) return false;
      const edit = { cancelled: false };
      shortcutEdit.current = edit;
      const previous = form.state.values.dictation_shortcut;
      let saveStarted = false;
      let failed = false;
      try {
        try {
          await configureDictationShortcut(value);
          if (!edit.cancelled) {
            saveStarted = true;
            await setSettingValues({ dictation_shortcut: value });
          }
          if (!edit.cancelled) {
            form.setFieldValue("dictation_shortcut", value);
            return true;
          }
        } catch {
          failed = true;
        }
        // A cancelled write must settle before restoring the previous value.
        const restored = await Promise.allSettled([
          configureDictationShortcut(previous),
          ...(saveStarted
            ? [setSettingValues({ dictation_shortcut: previous })]
            : []),
        ]);
        if (restored.some((result) => result.status === "rejected"))
          throw new Error(
            t`Couldn’t restore your previous shortcut. Try setting it again.`,
          );
        if (failed)
          throw new Error(
            t`Couldn’t save this shortcut. Your previous shortcut is unchanged.`,
          );
        return false;
      } finally {
        shortcutEdit.current = null;
        useDictationShortcut.setState({ capturing: false });
        capture.reset();
      }
    },
    onSuccess: (saved) => {
      if (saved && mounted.current) changeButton.current?.focus();
    },
  });
  const cancelCapture = (restoreFocus = false) => {
    capture.reset();
    if (shortcutEdit.current) {
      shortcutEdit.current.cancelled = true;
    } else {
      useDictationShortcut.setState({ capturing: false });
      shortcut.reset();
    }
    if (restoreFocus) changeButton.current?.focus();
  };
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      if (shortcutEdit.current) shortcutEdit.current.cancelled = true;
      else useDictationShortcut.setState({ capturing: false });
    };
  }, []);

  return (
    <section className="flex min-w-0 flex-col gap-4">
      <form.Field name="dictation_enabled">
        {(field) => (
          <div className="flex items-center justify-between gap-4">
            <div className="flex items-center gap-1.5">
              <label
                id="dictation-label"
                htmlFor="dictation-enabled"
                className="text-sm font-medium"
              >
                <Trans>Dictation</Trans>
              </label>
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    type="button"
                    aria-label={t`About dictation`}
                    className="text-muted-foreground hover:text-foreground focus-visible:ring-ring rounded-sm p-0.5 focus-visible:ring-2 focus-visible:outline-none"
                  >
                    <Info className="size-3.5" aria-hidden="true" />
                  </button>
                </TooltipTrigger>
                <TooltipContent className="max-w-64">
                  <Trans>
                    Uses your transcription model and language settings. No
                    meeting note is created.
                  </Trans>
                </TooltipContent>
              </Tooltip>
            </div>
            <Switch
              id="dictation-enabled"
              checked={field.state.value}
              disabled={busy || capturing || shortcutPending || save.isPending}
              onCheckedChange={(value) => {
                field.handleChange(value);
                void form.handleSubmit();
              }}
            />
          </div>
        )}
      </form.Field>
      <form.Subscribe selector={(state) => state.values.dictation_enabled}>
        {(enabled) => (
          <div className="space-y-3">
            <fieldset
              aria-labelledby="dictation-label"
              disabled={!enabled || busy || save.isPending}
              className="flex min-w-0 flex-col gap-4 disabled:opacity-60"
            >
              <form.Field name="dictation_shortcut">
                {(field) => (
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <span className="text-muted-foreground text-xs">
                      <Trans>Shortcut</Trans>
                    </span>
                    <div className="ml-auto flex max-w-full flex-wrap items-center justify-end gap-1.5">
                      {capturing && capture.isSuccess ? (
                        <Input
                          autoFocus
                          readOnly
                          value=""
                          aria-label={t`Press a new shortcut`}
                          placeholder={t`Press a new shortcut`}
                          className="bg-card h-7 w-48 max-w-full rounded-full text-xs shadow-none"
                          onBlur={() => cancelCapture()}
                          onKeyDown={(event) => {
                            if (event.key === "Tab") {
                              cancelCapture(true);
                              return;
                            }
                            event.preventDefault();
                            if (event.key === "Escape") {
                              cancelCapture(true);
                              return;
                            }
                            if (
                              [
                                "Control",
                                "Alt",
                                "Shift",
                                "Meta",
                                "Fn",
                              ].includes(event.key) ||
                              event.repeat ||
                              shortcutEdit.current !== null
                            )
                              return;
                            shortcut.mutate({
                              key: event.key,
                              ctrlKey: event.ctrlKey,
                              altKey: event.altKey,
                              shiftKey: event.shiftKey,
                              metaKey: event.metaKey,
                            });
                          }}
                        />
                      ) : (
                        <Kbd className="h-7 px-2 font-normal tracking-normal normal-case">
                          {field.state.value.replace(
                            /Cmd/g,
                            navigator.platform.includes("Mac") ? "Cmd" : "Win",
                          )}
                        </Kbd>
                      )}
                      <Button
                        ref={changeButton}
                        variant="outline"
                        size="sm"
                        className="bg-card rounded-full font-normal shadow-none"
                        disabled={capture.isPending || shortcutPending}
                        onMouseDown={(event) => {
                          if (capturing) event.preventDefault();
                        }}
                        onClick={() => {
                          if (capturing) {
                            cancelCapture(true);
                            return;
                          }
                          shortcut.reset();
                          capture.mutate();
                        }}
                      >
                        {capturing ? (
                          <Trans>Cancel</Trans>
                        ) : (
                          <Trans>Change shortcut</Trans>
                        )}
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-muted-foreground rounded-full font-normal"
                        disabled={capturing || shortcutPending}
                        onClick={() => {
                          shortcut.mutate({
                            key: " ",
                            ctrlKey: true,
                            altKey: true,
                            shiftKey: false,
                            metaKey: false,
                          });
                        }}
                      >
                        <Trans>Reset</Trans>
                      </Button>
                    </div>
                  </div>
                )}
              </form.Field>
              {capturing && (
                <p className="text-muted-foreground text-xs">
                  <Trans>
                    Press two modifier keys with a letter, number, or Space.
                    Escape cancels.
                  </Trans>
                </p>
              )}
              {shortcut.isError && (
                <p role="alert" className="text-destructive text-xs">
                  {shortcut.error.message}
                </p>
              )}
              {capture.isError && (
                <p role="alert" className="text-destructive text-xs">
                  <Trans>Couldn’t change the shortcut. Try again.</Trans>
                </p>
              )}
              <form.Field name="dictation_trigger_mode">
                {(field) => (
                  <div className="flex flex-wrap items-center justify-between gap-4">
                    <label
                      htmlFor="dictation-trigger-mode"
                      className="text-muted-foreground text-xs"
                    >
                      <Trans>Shortcut behavior</Trans>
                    </label>
                    <Select
                      value={field.state.value}
                      disabled={
                        !enabled ||
                        busy ||
                        save.isPending ||
                        capturing ||
                        shortcutPending
                      }
                      onValueChange={(value) => {
                        field.handleChange(value);
                        void form.handleSubmit();
                      }}
                    >
                      <SelectTrigger
                        id="dictation-trigger-mode"
                        className="bg-card w-48 max-w-full rounded-full shadow-none focus:ring-0"
                      >
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="hold">
                          <Trans>Hold to speak</Trans>
                        </SelectItem>
                        <SelectItem value="toggle">
                          <Trans>Press to start/stop</Trans>
                        </SelectItem>
                      </SelectContent>
                    </Select>
                  </div>
                )}
              </form.Field>
            </fieldset>
            {enabled && <DictationReadiness />}
          </div>
        )}
      </form.Subscribe>
      {save.isError && (
        <p role="alert" className="text-destructive text-xs">
          <Trans>Could not save dictation settings.</Trans>
        </p>
      )}
    </section>
  );
}

function DictationReadiness() {
  const { t } = useLingui();
  const mic = usePermission("microphone");
  const { current_stt_model, current_stt_provider } = useConfigValues([
    "current_stt_model",
    "current_stt_provider",
  ]);
  const model =
    current_stt_provider === "fmtr" &&
    isSupportedLocalSttModel(current_stt_model)
      ? current_stt_model
      : null;
  const downloaded = useQuery({
    queryKey: ["dictation-model-ready", model],
    enabled: Boolean(model),
    queryFn: async () => {
      if (!model) return false;
      const result = await localSttCommands.isModelDownloaded(model);
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
    refetchInterval: 3000,
  });
  const openNew = useTabs((state) => state.openNew);
  const permission = useMutation({
    mutationFn: async () => {
      if (mic.status === "neverRequested") mic.request();
      else await mic.open();
    },
  });
  if (mic.status && mic.status !== "authorized")
    return (
      <ReadinessAction
        title={t`Allow microphone access to dictate.`}
        label={t`Open Settings`}
        onClick={() => permission.mutate()}
      />
    );
  if (!model || downloaded.data === false || downloaded.isError)
    return (
      <ReadinessAction
        title={t`Choose a downloaded transcription model.`}
        label={t`Choose model`}
        onClick={() =>
          openNew({ type: "settings", state: { tab: "transcription" } })
        }
      />
    );
  return navigator.platform.includes("Mac") ? (
    <MacDictationReadiness />
  ) : (
    <ShortcutReadiness />
  );
}

function MacDictationReadiness() {
  const { t } = useLingui();
  const input = usePermission("inputMonitoring");
  const accessibility = usePermission("accessibility");
  const needed =
    input.status && input.status !== "authorized"
      ? input
      : accessibility.status && accessibility.status !== "authorized"
        ? accessibility
        : null;
  const permission = useMutation({
    mutationFn: async () => {
      if (needed?.status === "neverRequested") needed.request();
      else await needed?.open();
    },
  });
  if (needed)
    return (
      <ReadinessAction
        title={
          needed === input
            ? t`Allow Input Monitoring to use your shortcut.`
            : t`Allow Accessibility to insert your words.`
        }
        label={t`Open Settings`}
        onClick={() => permission.mutate()}
      />
    );
  return <ShortcutReadiness />;
}

function ShortcutReadiness() {
  const { t } = useLingui();
  const unavailable = useDictationShortcut((state) => state.unavailable);
  const openNew = useTabs((state) => state.openNew);
  return unavailable ? (
    <ReadinessAction
      title={t`Your dictation shortcut is unavailable.`}
      label={t`Permissions`}
      onClick={() =>
        openNew({ type: "settings", state: { tab: "permissions" } })
      }
    />
  ) : null;
}

function ReadinessAction({
  title,
  label,
  onClick,
}: {
  title: string;
  label: string;
  onClick: () => void;
}) {
  return (
    <div
      role="status"
      className="flex flex-wrap items-center justify-between gap-3 text-xs"
    >
      <span className="text-muted-foreground">{title}</span>
      <Button
        variant="outline"
        size="sm"
        className="bg-card rounded-full font-normal shadow-none"
        onClick={onClick}
      >
        {label}
      </Button>
    </div>
  );
}
