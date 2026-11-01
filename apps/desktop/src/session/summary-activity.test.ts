import { describe, expect, it } from "vitest";

import { beginSummaryActivity, useSummaryActivity } from "./summary-activity";

describe("summary activity ownership", () => {
  it("keeps the lock across capture, queue, and overlapping recording cycles", () => {
    const capture = beginSummaryActivity("session", "recording");
    expect(useSummaryActivity.getState().sessions).toEqual({
      session: "recording",
    });
    capture.setPhase("processing");
    const queue = beginSummaryActivity("session", "processing");
    capture.finish();
    expect(useSummaryActivity.getState().sessions.session).toBe("processing");
    const resumed = beginSummaryActivity("session", "recording");
    queue.finish();
    capture.setPhase("processing");
    capture.finish();
    expect(useSummaryActivity.getState().sessions.session).toBe("recording");
    resumed.finish();
    expect(useSummaryActivity.getState().sessions).toEqual({});
  });

  it("isolates sessions and lets one operation finish without releasing another", () => {
    const first = beginSummaryActivity("first", "recording");
    const second = beginSummaryActivity("second", "processing");
    first.finish();
    expect(useSummaryActivity.getState().sessions).toEqual({
      second: "processing",
    });
    second.finish();
  });
});
