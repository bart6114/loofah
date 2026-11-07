import { describe, expect, it } from "vitest";

import { deserializePinnedTabs } from "./pinned-persistence";
import { getDefaultState } from "./schema";

describe("pinned tab format", () => {
  it("drops retired and unsupported tab types while retaining current notes", () => {
    const pins = deserializePinnedTabs(
      JSON.stringify([
        { type: "daily", pinned: true },
        {
          type: "templates",
          state: { selectedMineId: "retired" },
          pinned: true,
        },
        { type: "chat_shortcuts", pinned: true },
        { type: "empty", pinned: true },
        { type: "sessions", id: "session-1", pinned: true },
      ]),
    );
    expect(pins).toMatchObject([{ type: "sessions", id: "session-1" }]);
  });

  it("retains legacy settings migration and normalizes retired settings tabs", () => {
    const pins = deserializePinnedTabs(
      JSON.stringify([
        { type: "settings", state: { tab: "todo" }, pinned: true },
        { type: "ai", state: { tab: "intelligence" }, pinned: true },
      ]),
    );
    expect(pins.map(getDefaultState)).toMatchObject([
      { type: "settings", state: { tab: "app" } },
      { type: "settings", state: { tab: "intelligence" } },
    ]);
  });

  it("ignores malformed storage and invalid array entries", () => {
    for (const value of ["not json", "null", "{}", '[null,1,"wrong"]'])
      expect(deserializePinnedTabs(value)).toEqual([]);
  });
});
