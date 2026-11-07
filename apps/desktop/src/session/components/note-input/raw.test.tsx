import { QueryClient } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { NoteEditorRef } from "@hypr/editor/note";

const state = vi.hoisted(() => ({
  host: null as any,
  props: [] as any[],
  ready: true,
  view: {},
  focus: vi.fn(),
  insert: vi.fn(),
  flush: vi.fn(),
  toast: vi.fn(),
  retry: vi.fn(),
  navigate: vi.fn(),
  attachment: vi.fn(),
}));
vi.mock("~/shared/application/vault-provider", () => ({
  useVaultHost: () => state.host,
}));
vi.mock("~/shared/application/vault-note-editor", async () => {
  const { useImperativeHandle, useEffect } = await import("react");
  return {
    VaultNoteEditor: (props: any) => {
      state.props.push(props);
      const ready = state.ready;
      useImperativeHandle(
        props.editorRef,
        () => ({
          get view() {
            return state.ready ? state.view : null;
          },
          commands: { focus: state.focus, insertAtStartAndFocus: state.insert },
          flushPendingChanges: state.flush,
        }),
        [],
      );
      const onReady = props.editorOptions.onViewReady;
      useEffect(() => {
        if (ready) onReady(state.view);
      }, [ready, onReady]);
      return <div>Shared vault note editor</div>;
    },
  };
});
vi.mock("@hypr/editor/note", () => ({
  useMentionNavigation: () => state.navigate,
}));
vi.mock("@hypr/ui/components/ui/toast", () => ({
  sonnerToast: { error: state.toast },
}));
vi.mock("~/editor-bridge/mention-config", () => ({
  useMentionConfig: () => ({ users: [] }),
}));
vi.mock("~/session/hooks/useAttachmentResolver", () => ({
  useAttachmentResolver: () => state.attachment,
}));
vi.mock("~/editor-bridge/app-link-view", () => ({ AppLinkView: () => null }));
vi.mock("~/editor-bridge/session-view", () => ({
  SessionNodeView: () => null,
}));
vi.mock("~/editor-bridge/open-editor-link", () => ({
  openEditorLink: vi.fn(),
}));
vi.mock("~/editor-bridge/session-mention-drop", () => ({
  sessionMentionDropConfig: { read: () => null },
}));
vi.mock("~/shared/application/device-recovery", () => ({
  DeviceRecovery: () => <div>Device recovery</div>,
}));

import { RawEditor } from "./raw";

beforeEach(() => {
  vi.clearAllMocks();
  state.props.length = 0;
  state.ready = true;
  state.host = {
    scope: {
      client: { scope: "local:fixture" },
      queryClient: new QueryClient(),
    },
    error: null,
    retry: state.retry,
  };
});
afterEach(cleanup);
const props = { sessionId: "session", rawMd: "", sessionTitle: "Title" };

describe("main raw note adapter", () => {
  it("supplies existing device/editor capabilities to the shared editor without enabling independent task writes", () => {
    const fileHandler = { onPaste: vi.fn() };
    render(
      <RawEditor
        {...props}
        className="custom"
        fileHandlerConfig={fileHandler}
      />,
    );
    const value = state.props[state.props.length - 1];
    expect(value.scope).toBe(state.host.scope);
    expect(value.showSaveControls).toBe(false);
    expect(value.editorOptions).toMatchObject({
      className: "session-note-editor custom",
      fileHandlerConfig: fileHandler,
      resolveAttachment: state.attachment,
      onMentionNavigate: state.navigate,
      syncContentWhenFocused: false,
    });
    expect(value.editorOptions.taskSource).toBeUndefined();
  });
  it("shows a persistent failure toast when the shared save fails", () => {
    render(<RawEditor {...props} />);
    state.props[state.props.length - 1].onSaveError(new Error("disk full"));
    expect(state.toast).toHaveBeenCalledWith(
      "Note is NOT being saved: disk full",
      expect.objectContaining({ id: "note-save-failed:local:fixture:session" }),
    );
  });
  it("retains an early focus request until the coherent editor has mounted", async () => {
    state.ready = false;
    const ref = createRef<NoteEditorRef>();
    const mounted = render(<RawEditor {...props} ref={ref} />);
    act(() => ref.current!.commands.focus());
    expect(state.focus).not.toHaveBeenCalled();
    state.ready = true;
    mounted.rerender(<RawEditor {...props} ref={ref} />);
    await waitFor(() => expect(state.focus).toHaveBeenCalledOnce());
    act(() => ref.current!.flushPendingChanges());
    expect(state.flush).toHaveBeenCalledOnce();
  });
  it("does not deliver another note's deferred commands after a session switch", async () => {
    state.ready = false;
    const ref = createRef<NoteEditorRef>();
    const mounted = render(<RawEditor {...props} ref={ref} />);
    act(() => ref.current!.commands.focus());
    state.ready = true;
    mounted.rerender(<RawEditor {...props} sessionId="other" ref={ref} />);
    await act(async () => {
      await Promise.resolve();
    });
    expect(state.focus).not.toHaveBeenCalled();
    expect(state.props[state.props.length - 1].sessionId).toBe("other");
  });
  it("shows a retryable bootstrap error without opening a different vault", () => {
    state.host = {
      scope: null,
      error: new Error("Vault startup failed"),
      retry: state.retry,
    };
    render(<RawEditor {...props} />);
    expect(screen.getByRole("alert").textContent).toContain(
      "Vault startup failed",
    );
    expect(state.props).toHaveLength(0);
    screen.getByRole("button", { name: "Retry" }).click();
    expect(state.retry).toHaveBeenCalledOnce();
  });
});
