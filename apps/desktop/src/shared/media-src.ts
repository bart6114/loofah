import { convertFileSrc } from "@tauri-apps/api/core";

export function mediaSrc(path: string) {
  return /^https?:\/\//.test(path) ? path : convertFileSrc(path);
}
