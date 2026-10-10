import { cn } from "@hypr/utils";

import mark from "../../../src-tauri/icons/src/loofah-mark-1024.png";

export function ThemeLogo({ className = "" }: { className?: string }) {
  return (
    <span aria-hidden="true" className={cn(["theme-logo", className])}>
      <span
        style={{ maskImage: `url(${mark})`, WebkitMaskImage: `url(${mark})` }}
      />
    </span>
  );
}

export function SidebarBrand() {
  return (
    <div data-tauri-drag-region className="sidebar-brand">
      <ThemeLogo />
      <span>loofah</span>
    </div>
  );
}
