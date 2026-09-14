import { useIndexQuery } from "~/shared/index-query";
import { commands, type Result } from "~/types/tauri.gen";

export type Tag = {
  id: string;
  name: string;
};

const tagsQueryKey = ["tags"];

function unwrap<T>(result: Result<T, string>): T {
  if (result.status === "error") {
    throw new Error(result.error);
  }
  return result.data;
}

export async function listTags(): Promise<Tag[]> {
  const items = unwrap(await commands.tagsList());
  return items.map((item) => ({ id: item.id, name: item.name ?? item.id }));
}

export function useTags(): Tag[] {
  const { data = EMPTY_TAGS } = useIndexQuery({
    entity: "tags",
    queryKey: tagsQueryKey,
    queryFn: listTags,
  });

  return data;
}

export async function ensureTag(name: string): Promise<Tag> {
  const item = unwrap(await commands.tagsEnsure(name));
  return { id: item.id, name: item.name ?? item.id };
}

// Tags attached to any session, whether or not they made it into the registry
// (pre-registry sessions, hand-edited `_meta.json`). Unioned with `useTags()`
// for typeahead so nothing in the vault is unsuggestable.
export function useInUseTags(): string[] {
  const { data = EMPTY_NAMES } = useIndexQuery({
    entity: "session_headers",
    queryKey: ["in-use-tags"],
    queryFn: async () => {
      const result = await commands.sessionListHeaders();
      if (result.status === "error") {
        throw new Error(result.error);
      }
      const names = new Set<string>();
      for (const entry of result.data) {
        for (const tag of entry.tags) {
          names.add(tag);
        }
      }
      return [...names].sort();
    },
  });
  return data;
}

const EMPTY_TAGS: Tag[] = [];
const EMPTY_NAMES: string[] = [];
