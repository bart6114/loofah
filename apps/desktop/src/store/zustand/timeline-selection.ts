import { createContext, useContext } from "react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

interface TimelineSelectionState {
  selectedIds: string[];
  anchorId: string | null;
  setAnchor: (id: string) => void;
  toggleSelect: (id: string) => void;
  selectRange: (flatItemKeys: string[], targetId: string) => void;
  selectAll: (ids: string[]) => void;
  clear: () => void;
  isSelected: (id: string) => boolean;
}

export function createTimelineSelectionStore() {
  let active = true;
  const store = createStore<TimelineSelectionState>()((set, get) => {
    const update = (patch: Partial<TimelineSelectionState>) => {
      if (active) set(patch);
    };
    return {
      selectedIds: [],
      anchorId: null,
      setAnchor: (id) => update({ anchorId: id, selectedIds: [] }),
      toggleSelect: (id) => {
        const { selectedIds, anchorId } = get();
        if (selectedIds.includes(id)) {
          const filtered = selectedIds.filter((s) => s !== id);
          update({
            selectedIds: filtered,
            anchorId: filtered.length > 0 ? anchorId : null,
          });
        } else {
          const base =
            selectedIds.length === 0 && anchorId && anchorId !== id
              ? [anchorId]
              : selectedIds;
          update({
            selectedIds: [...base, id],
            anchorId: anchorId ?? id,
          });
        }
      },
      selectRange: (flatItemKeys, targetId) => {
        const { anchorId } = get();
        if (!anchorId) {
          update({ anchorId: targetId, selectedIds: [targetId] });
          return;
        }

        const anchorIndex = flatItemKeys.indexOf(anchorId);
        const targetIndex = flatItemKeys.indexOf(targetId);

        if (anchorIndex === -1 || targetIndex === -1) {
          update({ anchorId: targetId, selectedIds: [targetId] });
          return;
        }

        const start = Math.min(anchorIndex, targetIndex);
        const end = Math.max(anchorIndex, targetIndex);
        const range = flatItemKeys.slice(start, end + 1);

        update({ selectedIds: range });
      },
      selectAll: (ids) => {
        const { anchorId } = get();
        update({
          selectedIds: ids,
          anchorId:
            anchorId && ids.includes(anchorId) ? anchorId : (ids[0] ?? null),
        });
      },
      clear: () => update({ selectedIds: [], anchorId: null }),
      isSelected: (id) => get().selectedIds.includes(id),
    };
  });
  return {
    ...store,
    dispose() {
      if (!active) return;
      active = false;
      store.setState(store.getInitialState(), true);
    },
  };
}

export const TimelineSelectionContext = createContext<ReturnType<
  typeof createTimelineSelectionStore
> | null>(null);

export function useTimelineSelectionStore() {
  const store = useContext(TimelineSelectionContext);
  if (!store)
    throw new Error("Timeline selection requires an immutable tab host");
  return store;
}

export function useTimelineSelection<T>(
  selector: (state: TimelineSelectionState) => T,
): T {
  return useStore(useTimelineSelectionStore(), selector);
}
