import { Trans, useLingui } from "@lingui/react/macro";
import { useMutation } from "@tanstack/react-query";
import { CheckIcon, MoonIcon } from "lucide-react";
import { type CSSProperties, useMemo } from "react";

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@hypr/ui/components/ui/select";
import { sonnerToast } from "@hypr/ui/components/ui/toast";

import { setSettingValues } from "~/settings/queries";
import { useConfigValue } from "~/shared/config";
import { normalizeThemePreference } from "~/shared/theme/apply";
import {
  DESIGN_THEMES,
  designThemeTokens,
  getDesignTheme,
} from "~/shared/theme/catalog";
import { ThemeLogo } from "~/shared/theme/logo";
import {
  applyDesignThemePreference,
  applyThemePreference,
} from "~/shared/theme/provider";
import type { ThemePreference } from "~/shared/theme/resolve";

export function ThemeSelector() {
  const { t } = useLingui();
  const appearance = normalizeThemePreference(useConfigValue("theme"));
  const designTheme = getDesignTheme(useConfigValue("design_theme"));
  const mutation = useMutation({
    mutationFn: (values: { design_theme?: string; theme?: ThemePreference }) =>
      setSettingValues(values),
    onMutate: (values) => {
      applyDesignThemePreference(values.design_theme ?? designTheme.id);
      void applyThemePreference(values.theme ?? appearance);
    },
    onError: () => {
      applyDesignThemePreference(designTheme.id);
      void applyThemePreference(appearance);
      sonnerToast.error(t`Couldn't save your appearance. Please try again.`);
    },
  });
  const selectedId = mutation.isPending
    ? (mutation.variables?.design_theme ?? designTheme.id)
    : designTheme.id;
  const selectedAppearance = mutation.isPending
    ? (mutation.variables?.theme ?? appearance)
    : appearance;
  const options = useMemo(
    () => [
      { value: "light", label: t`Light` },
      { value: "dark", label: t`Dark` },
      { value: "system", label: t`Use system setting` },
    ],
    [t],
  );

  return (
    <section
      className="flex flex-col gap-5"
      aria-label={t`Appearance`}
      aria-busy={mutation.isPending}
    >
      {getDesignTheme(selectedId).appearance === "adaptive" ? (
        <div className="flex flex-row items-center justify-between gap-4">
          <div>
            <h3 className="mb-1 text-sm font-medium">
              <Trans>Appearance</Trans>
            </h3>
            <p className="text-muted-foreground text-xs">
              <Trans>Choose light, dark, or match your system setting.</Trans>
            </p>
          </div>
          <Select
            value={selectedAppearance}
            disabled={mutation.isPending}
            onValueChange={(theme) =>
              mutation.mutate({ theme: theme as ThemePreference })
            }
          >
            <SelectTrigger
              aria-label={t`Appearance`}
              className="bg-card w-48 shadow-none focus:ring-0"
            >
              <SelectValue placeholder={t`Select appearance`} />
            </SelectTrigger>
            <SelectContent>
              {options.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      ) : null}
      <div>
        <h3 className="mb-1 text-sm font-medium">
          <Trans>Design theme</Trans>
        </h3>
        <p className="text-muted-foreground mb-4 text-xs">
          <Trans>
            Choose a style for your workspace. Default follows your appearance
            setting.
          </Trans>
        </p>
        <div className="theme-picker-grid">
          {DESIGN_THEMES.map((theme) => {
            const dark = theme.appearance === "dark";
            return (
              <button
                key={theme.id}
                type="button"
                className="theme-picker-card"
                aria-label={theme.name}
                aria-pressed={theme.id === selectedId}
                disabled={mutation.isPending}
                onClick={() =>
                  mutation.mutate({
                    design_theme: theme.id,
                  })
                }
              >
                {theme.appearance === "adaptive" ? (
                  <span className="theme-swatch-split" aria-hidden="true">
                    <ThemeSwatch theme={theme} dark={false} />
                    <ThemeSwatch theme={theme} dark />
                  </span>
                ) : (
                  <ThemeSwatch theme={theme} dark={dark} />
                )}
                <span className="flex items-center justify-between gap-2 px-1 text-sm font-medium">
                  {theme.name}
                  {theme.id === selectedId ? (
                    <CheckIcon size={14} aria-hidden="true" />
                  ) : dark ? (
                    <MoonIcon size={13} aria-hidden="true" />
                  ) : null}
                </span>
                <span className="text-muted-foreground px-1 pb-1 text-xs leading-relaxed">
                  {theme.description}
                </span>
              </button>
            );
          })}
        </div>
      </div>
    </section>
  );
}

function ThemeSwatch({
  theme,
  dark,
}: {
  theme: (typeof DESIGN_THEMES)[number];
  dark: boolean;
}) {
  return (
    <span
      className="theme-swatch"
      aria-hidden="true"
      style={designThemeTokens(theme, dark) as CSSProperties}
    >
      <span className="theme-swatch-sidebar">
        <ThemeLogo />
      </span>
      <span className="theme-swatch-note">
        <strong>Notes.</strong>
        <i />
        <i />
      </span>
    </span>
  );
}
