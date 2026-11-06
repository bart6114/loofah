import {
  createContext,
  useContext,
  type ComponentType,
  type ReactNode,
} from "react";

import {
  legacyAttachmentPresentation,
  type AttachmentAction,
  type AttachmentPresentation,
} from "./attachment-presentation";

export type AttachmentResolution = {
  path?: string;
  open?: AttachmentAction;
  reveal?: AttachmentAction;
  save?: AttachmentAction;
  src?: string;
  labels?: { open: string; reveal: string; save?: string; retry: string };
  preview?: { loading: string; error: string; unavailable: string };
  pending?: boolean;
  feedback?: { message: string; error?: boolean };
  retry?: AttachmentAction;
};

export type AttachmentResourceLoader = ComponentType<{
  attachmentId: string;
  children: (resolved: AttachmentResolution | null) => ReactNode;
}>;

export const AttachmentResourceContext =
  createContext<AttachmentResourceLoader | null>(null);

export function AttachmentResource({
  attachmentId,
  children,
}: {
  attachmentId: unknown;
  children: (resolved: AttachmentResolution | null) => ReactNode;
}) {
  const Loader = useContext(AttachmentResourceContext);
  const resolve = useAttachmentResolver();
  if (typeof attachmentId !== "string") return children(null);
  if (Loader) return <Loader attachmentId={attachmentId}>{children}</Loader>;
  return children(resolve?.(attachmentId) ?? null);
}

export type AttachmentResolver = (
  attachmentId: string,
) => AttachmentResolution | null;

export const AttachmentResolverContext =
  createContext<AttachmentResolver | null>(null);
export const AttachmentPresentationContext =
  createContext<AttachmentPresentation>(legacyAttachmentPresentation);
export function useAttachmentPresentation() {
  return useContext(AttachmentPresentationContext);
}

export const AttachmentEditingContext = createContext(true);

export function useAttachmentResolver() {
  return useContext(AttachmentResolverContext);
}

export function useAttachmentEditingEnabled() {
  return useContext(AttachmentEditingContext);
}
