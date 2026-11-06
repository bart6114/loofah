import { createContext, useContext } from "react";

import {
  legacyAttachmentPresentation,
  type AttachmentAction,
  type AttachmentPresentation,
} from "./attachment-presentation";

export type AttachmentResolution = {
  path?: string;
  open?: AttachmentAction;
  reveal?: AttachmentAction;
  src: string;
};

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
