import { cn } from "@hypr/utils";

import mark from "../../../src-tauri/icons/src/loofah-mark-1024.png";
import { getDesignTheme } from "./catalog";

import { useConfigValue } from "~/shared/config";

export function ThemeLogo({
  themeId,
  className = "",
}: {
  themeId: string;
  className?: string;
}) {
  return (
    <span
      aria-hidden="true"
      className={cn([
        "theme-logo",
        themeId === "default" && "theme-logo-default",
        className,
      ])}
    >
      {themeId === "default" ? (
        <img src="/assets/app-icon.png" alt="" />
      ) : (
        <span
          style={{ maskImage: `url(${mark})`, WebkitMaskImage: `url(${mark})` }}
        />
      )}
    </span>
  );
}

export function SidebarBrand() {
  const theme = getDesignTheme(useConfigValue("design_theme"));

  return (
    <div data-tauri-drag-region className="sidebar-brand">
      <ThemeLogo themeId={theme.id} />
      <span>loofah</span>
    </div>
  );
}
