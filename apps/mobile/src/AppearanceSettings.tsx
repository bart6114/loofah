import { useMutation, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { Check, Moon, Sun, Smartphone } from "lucide-react";
import type { CSSProperties } from "react";

import {
  DESIGN_THEMES,
  designThemeTokens,
  getDesignTheme,
  resolveThemeAppearance,
} from "@hypr/design-theme/catalog";
import { cn } from "@hypr/utils";

import { errorMessage, snapshotOptions, type Snapshot } from "./api";
import { applyAppearance, normalizeAppearance } from "./appearance";

export function AppearanceSettings({ snapshot }: { snapshot: Snapshot }) {
  const client = useQueryClient();
  const saved = normalizeAppearance(snapshot.appearance);
  const apply = (value: Snapshot["appearance"]) => {
    applyAppearance(value);
    void invoke("mobile_apply_appearance", {
      appearance: resolveThemeAppearance(
        getDesignTheme(value.design_theme),
        value.theme,
      ),
    }).catch(console.error);
  };
  const save = useMutation({
    mutationFn: (value: Snapshot["appearance"]) =>
      invoke("mobile_save_appearance", {
        theme: value.theme,
        designTheme: value.design_theme,
      }),
    onMutate: apply,
    onSuccess: (_, value) => {
      client.setQueryData<Snapshot>(snapshotOptions.queryKey, (previous) =>
        previous ? { ...previous, appearance: value } : previous,
      );
      void client.invalidateQueries({ queryKey: snapshotOptions.queryKey });
    },
    onError: () => apply(saved),
  });
  const selected = save.isPending ? save.variables : saved;
  const theme = getDesignTheme(selected.design_theme);
  return (
    <section
      className="appearance-settings"
      aria-label="Appearance"
      aria-busy={save.isPending}
    >
      <p className="settings-intro">Make a little space your own.</p>
      {theme.appearance === "adaptive" ? (
        <fieldset className="appearance-modes" disabled={save.isPending}>
          <legend>Appearance</legend>
          {(
            [
              ["system", "System", Smartphone],
              ["light", "Light", Sun],
              ["dark", "Dark", Moon],
            ] as const
          ).map(([value, label, Icon]) => (
            <label
              key={value}
              className={cn([selected.theme === value && "selected"])}
            >
              <input
                type="radio"
                name="appearance"
                value={value}
                checked={selected.theme === value}
                onChange={() => save.mutate({ ...selected, theme: value })}
              />
              <Icon size={20} aria-hidden="true" />
              <span>{label}</span>
            </label>
          ))}
        </fieldset>
      ) : (
        <p className="caption">
          {theme.name} has a {theme.appearance} appearance. Choose Default to
          follow your iPhone’s setting.
        </p>
      )}
      <h2 className="section-label">Design theme</h2>
      <div className="mobile-theme-grid">
        {DESIGN_THEMES.map((item) => {
          const active = item.id === selected.design_theme;
          return (
            <button
              type="button"
              key={item.id}
              className="mobile-theme-card"
              aria-label={item.name}
              aria-describedby={`theme-${item.id}-description`}
              aria-pressed={active}
              disabled={save.isPending}
              onClick={() =>
                save.mutate({ ...selected, design_theme: item.id })
              }
            >
              <span
                className="mobile-theme-preview"
                style={
                  designThemeTokens(
                    item,
                    item.appearance === "dark" ||
                      (item.appearance === "adaptive" &&
                        document.documentElement.classList.contains("dark")),
                  ) as CSSProperties
                }
                aria-hidden="true"
              >
                <span className="theme-preview-bar">
                  <i />
                  <i />
                  <i />
                </span>
                <span className="theme-preview-page">
                  <strong>
                    A little
                    <br />
                    perspective.
                  </strong>
                  <i />
                  <i />
                  <i />
                </span>
                <span className="theme-preview-record" />
              </span>
              <span className="theme-card-title">
                {item.name}
                {active && <Check size={17} aria-hidden="true" />}
              </span>
              <span
                id={`theme-${item.id}-description`}
                className="theme-card-description"
              >
                {item.description}
              </span>
            </button>
          );
        })}
      </div>
      {save.isError && (
        <p className="error" role="alert">
          Couldn’t save your appearance. {errorMessage(save.error)}
        </p>
      )}
      <p className="caption">
        Your theme is saved with your vault and shared with your other Loofah
        devices when you sync.
      </p>
    </section>
  );
}
