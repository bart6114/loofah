import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const openNew = vi.hoisted(() => vi.fn());

vi.mock("~/store/zustand/tabs", () => ({
  useTabs: (selector: (state: { openNew: typeof openNew }) => unknown) =>
    selector({ openNew }),
}));

import { ConfigError } from "./config-error";

describe("ConfigError", () => {
  afterEach(() => {
    cleanup();
    openNew.mockReset();
  });

  it("offers Intelligence setup from the empty summary state", () => {
    render(<ConfigError sessionTitle="Weekly sync" />);

    expect(screen.getByRole("alert")).not.toBeNull();
    expect(screen.getByText("Set up AI summaries")).not.toBeNull();
    expect(
      screen.getByText(
        "Connect an Intelligence provider to generate a summary from your notes or transcript.",
      ),
    ).not.toBeNull();

    fireEvent.click(
      screen.getByRole("button", { name: "Set up Intelligence" }),
    );
    expect(openNew).toHaveBeenNthCalledWith(1, {
      type: "settings",
      state: { tab: "intelligence" },
    });
  });

  it("keeps the session title and speakers row visible", () => {
    const trailer = document.createElement("div");
    trailer.textContent = "Alice";

    render(
      <ConfigError sessionTitle="Weekly sync" titleTrailerElement={trailer} />,
    );

    expect(
      screen.getByRole("heading", { level: 1, name: "Weekly sync" }),
    ).not.toBeNull();
    expect(screen.getByText("Alice")).not.toBeNull();
  });

  it("falls back to the untitled placeholder", () => {
    render(<ConfigError sessionTitle="  " />);

    expect(
      screen.getByRole("heading", { level: 1, name: "Untitled" }),
    ).not.toBeNull();
  });
});
