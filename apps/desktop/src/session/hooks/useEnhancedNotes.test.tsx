import { renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
const mocks = vi.hoisted(() => ({ notes: [] as Array<{ id: string }> }));
vi.mock("~/session/queries", () => ({
  useEnhancedNoteRecords: () => mocks.notes,
}));
import { useEnhancedNotes } from "./useEnhancedNotes";

describe("useEnhancedNotes", () => {
  it("reads existing documents without creating a default document", () => {
    const { result, rerender } = renderHook(() =>
      useEnhancedNotes("session-1"),
    );
    expect(result.current).toEqual([]);
    mocks.notes = [{ id: "summary-1" }];
    rerender();
    expect(result.current).toEqual(["summary-1"]);
  });
});
