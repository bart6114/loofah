import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  getTemplateSource: vi.fn(),
  renderTemplate: vi.fn(),
  setSettingValue: vi.fn(),
  values: {
    auto_summary_prompt: "",
    selected_template_id: "",
  } as Record<string, string>,
}));

vi.mock("@lingui/react/macro", () => ({
  Trans: ({ children }: { children?: ReactNode }) => <>{children}</>,
  useLingui: () => ({
    t: (
      input: TemplateStringsArray | { message?: string } | string,
      ...values: unknown[]
    ) => {
      if (typeof input === "string") return input;
      if (Array.isArray(input)) {
        return (input as readonly string[]).reduce(
          (message: string, part: string, index: number) =>
            `${message}${part}${index < values.length ? String(values[index]) : ""}`,
          "",
        );
      }
      return (input as { message?: string }).message ?? "";
    },
  }),
}));

vi.mock("@hypr/editor/prompt", async () => {
  const React = await import("react");

  return {
    PromptEditor: React.forwardRef(function PromptEditorMock(
      {
        ariaLabel,
        initialValue,
        onBlur,
        onChange,
      }: {
        ariaLabel: string;
        initialValue: string;
        onBlur?: () => void;
        onChange: (value: string) => void;
      },
      ref: React.ForwardedRef<{
        insertToken: (name: string) => void;
        setValue: (value: string) => void;
      }>,
    ) {
      const [value, setValue] = React.useState(initialValue);
      const update = (next: string) => {
        setValue(next);
        onChange(next);
      };

      React.useImperativeHandle(ref, () => ({
        insertToken: (name) => update(`${value}\n{{ ${name} }}`),
        setValue: update,
      }));

      return (
        <textarea
          aria-label={ariaLabel}
          value={value}
          onBlur={onBlur}
          onChange={(event) => update(event.target.value)}
        />
      );
    }),
  };
});

vi.mock("@hypr/plugin-template", () => ({
  commands: {
    getTemplateSource: mocks.getTemplateSource,
    render: mocks.renderTemplate,
  },
}));

vi.mock("~/settings/queries", () => ({
  setSettingValue: mocks.setSettingValue,
}));

vi.mock("~/shared/config", () => ({
  useConfigValue: (key: string) => mocks.values[key] ?? "",
}));

import { SummaryPromptForm, SummaryPromptSettings } from "./summary-prompt";

const defaultPrompt =
  "Today is {{ current_date }}. Write the summary in {{ language }}.";

function renderWithQueryClient(node: ReactNode) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>{node}</QueryClientProvider>,
  );
}

describe("Summary prompt editor", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.values.auto_summary_prompt = "";
    mocks.values.selected_template_id = "";
    mocks.getTemplateSource.mockResolvedValue({
      status: "ok",
      data: defaultPrompt,
    });
    mocks.renderTemplate.mockResolvedValue({ status: "ok", data: "rendered" });
    mocks.setSettingValue.mockResolvedValue(undefined);
  });

  afterEach(cleanup);

  it("loads the built-in source and shows supported variables and context", async () => {
    renderWithQueryClient(<SummaryPromptSettings initiallyOpen />);

    expect(
      (await screen.findByRole("textbox", {
        name: "Summary prompt",
      })) as HTMLTextAreaElement,
    ).toHaveProperty("value", defaultPrompt);
    expect(screen.getByRole("button", { name: /Current date/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /Language/ })).toBeTruthy();
    expect(screen.getByText("Context always provided")).toBeTruthy();
  });

  it("inserts supported variables as canonical prompt tokens", () => {
    renderWithQueryClient(
      <SummaryPromptForm
        initiallyOpen
        defaultPrompt={defaultPrompt}
        promptOverride=""
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /Language/ }));

    expect(
      screen.getByRole("textbox", {
        name: "Summary prompt",
      }) as HTMLTextAreaElement,
    ).toHaveProperty("value", `${defaultPrompt}\n{{ language }}`);
  });

  it("validates and saves a customized prompt", async () => {
    renderWithQueryClient(
      <SummaryPromptForm
        initiallyOpen
        defaultPrompt={defaultPrompt}
        promptOverride=""
      />,
    );

    fireEvent.change(screen.getByRole("textbox", { name: "Summary prompt" }), {
      target: { value: "Write in {{ language }}." },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(mocks.renderTemplate).toHaveBeenCalledWith({
        enhanceSystem: {
          language: "en",
          promptOverride: "Write in {{ language }}.",
        },
      }),
    );
    expect(mocks.setSettingValue).toHaveBeenCalledWith(
      "auto_summary_prompt",
      "Write in {{ language }}.",
    );
  });

  it("stores the default-equivalent source as an empty override", async () => {
    renderWithQueryClient(
      <SummaryPromptForm
        initiallyOpen
        defaultPrompt={defaultPrompt}
        promptOverride="Custom"
      />,
    );

    fireEvent.change(screen.getByRole("textbox", { name: "Summary prompt" }), {
      target: { value: `  ${defaultPrompt}\n` },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(mocks.setSettingValue).toHaveBeenCalledWith(
        "auto_summary_prompt",
        "",
      ),
    );
  });

  it("shows Jinja validation errors without saving", async () => {
    mocks.renderTemplate.mockResolvedValue({
      status: "error",
      error: "unknown variables: customer",
    });
    renderWithQueryClient(
      <SummaryPromptForm
        initiallyOpen
        defaultPrompt={defaultPrompt}
        promptOverride=""
      />,
    );

    fireEvent.change(screen.getByRole("textbox", { name: "Summary prompt" }), {
      target: { value: "Hello {{ customer }}" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect((await screen.findByRole("alert")).textContent).toContain(
      "unknown variables: customer",
    );
    expect(mocks.setSettingValue).not.toHaveBeenCalled();
  });

  it("resets a customized prompt to the built-in source", async () => {
    renderWithQueryClient(
      <SummaryPromptForm
        initiallyOpen
        defaultPrompt={defaultPrompt}
        promptOverride="Custom prompt"
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Reset to default" }));
    expect(mocks.setSettingValue).not.toHaveBeenCalled();
    expect(
      screen.getByRole("textbox", { name: "Summary prompt" }),
    ).toHaveProperty("value", defaultPrompt);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(mocks.setSettingValue).toHaveBeenCalledWith(
        "auto_summary_prompt",
        "",
      ),
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("keeps instructions compact until customization is opened", async () => {
    renderWithQueryClient(<SummaryPromptSettings />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Customize instructions" }),
    );
    expect(screen.getByRole("dialog")).toBeTruthy();
    expect(
      screen.getByRole("textbox", { name: "Summary prompt" }),
    ).toHaveProperty("value", defaultPrompt);
  });

  it("protects unsaved edits on cancel and discards only when requested", () => {
    renderWithQueryClient(
      <SummaryPromptForm
        initiallyOpen
        defaultPrompt={defaultPrompt}
        promptOverride=""
      />,
    );
    fireEvent.change(screen.getByRole("textbox", { name: "Summary prompt" }), {
      target: { value: "Short summaries" },
    });
    expect(screen.getByRole("status").textContent).toBe("Unsaved changes");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
    expect(
      screen.getByRole("textbox", { name: "Summary prompt" }),
    ).toHaveProperty("value", "Short summaries");
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    fireEvent.click(screen.getByRole("button", { name: "Discard changes" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.click(
      screen.getByRole("button", { name: "Customize instructions" }),
    );
    expect(
      screen.getByRole("textbox", { name: "Summary prompt" }),
    ).toHaveProperty("value", defaultPrompt);
    expect(mocks.setSettingValue).not.toHaveBeenCalled();
  });

  it("retains an unsaved draft when settings unmount during navigation", () => {
    const queryClient = new QueryClient();
    const node = (show: boolean) => (
      <QueryClientProvider client={queryClient}>
        {show && (
          <SummaryPromptForm
            initiallyOpen
            defaultPrompt={defaultPrompt}
            promptOverride=""
          />
        )}
      </QueryClientProvider>
    );
    const { rerender } = render(node(true));
    fireEvent.change(screen.getByRole("textbox", { name: "Summary prompt" }), {
      target: { value: "Keep this draft" },
    });
    rerender(node(false));
    rerender(node(true));
    expect(
      screen.getByRole("textbox", { name: "Summary prompt" }),
    ).toHaveProperty("value", "Keep this draft");
    expect(screen.getByRole("status").textContent).toBe("Unsaved changes");
  });
});
