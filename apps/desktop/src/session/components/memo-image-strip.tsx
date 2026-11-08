import { useLingui } from "@lingui/react/macro";
import { useMemo } from "react";

import { AttachmentResource } from "@hypr/editor/node-views";
import { cn } from "@hypr/utils";

import { useSessionRawMd } from "~/session/queries";
import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";
import { VaultAttachmentProvider } from "~/shared/application/vault-attachment-provider";

type MemoImage = {
  key: string;
  attachmentId: string | null;
  src: string | null;
  alt: string;
};

function collectMemoImages(rawMd: string | null): MemoImage[] {
  if (!rawMd) {
    return [];
  }

  let doc: unknown;
  try {
    doc = JSON.parse(rawMd);
  } catch {
    return [];
  }

  const images: MemoImage[] = [];
  const seen = new Set<string>();

  const visit = (value: unknown) => {
    if (!value || typeof value !== "object") {
      return;
    }

    const node = value as {
      type?: unknown;
      attrs?: Record<string, unknown>;
      content?: unknown[];
    };
    if (node.type === "image") {
      const attachmentId =
        typeof node.attrs?.attachmentId === "string" && node.attrs.attachmentId
          ? node.attrs.attachmentId
          : null;
      const src =
        typeof node.attrs?.src === "string" && node.attrs.src
          ? node.attrs.src
          : null;
      const key = attachmentId ?? src;
      if (key && !seen.has(key)) {
        seen.add(key);
        images.push({
          key,
          attachmentId,
          src,
          alt: typeof node.attrs?.alt === "string" ? node.attrs.alt : "",
        });
      }
    }

    node.content?.forEach(visit);
  };

  visit(doc);
  return images;
}

/// Thumbnails of the images embedded in the session's memo, so screenshots
/// stay visible on the summary view without switching to the memo tab.
export function MemoImageStrip({
  sessionId,
  className,
  onImageClick,
}: {
  sessionId: string;
  className?: string;
  onImageClick?: (src: string, attachmentId?: string) => void;
}) {
  const { t } = useLingui();
  const rawMd = useSessionRawMd(sessionId);
  const owner = useVaultReadScope();
  const images = useMemo(() => collectMemoImages(rawMd), [rawMd]);

  if (!owner.readable || images.length === 0) return null;
  return (
    <VaultAttachmentProvider scope={owner.scope} sessionId={sessionId}>
      <div className={cn(["flex flex-wrap items-center gap-2", className])}>
        {images.map((image) => (
          <AttachmentResource key={image.key} attachmentId={image.attachmentId}>
            {(resolved) => {
              const src = image.attachmentId
                ? resolved?.src
                : owner.scope.client.attachmentPresentation({ src: image.src })
                    .src;
              if (!src) return null;
              return (
                <button
                  type="button"
                  aria-label={t`Show image in note`}
                  onClick={() => {
                    if (owner.current())
                      onImageClick?.(src, image.attachmentId ?? undefined);
                  }}
                  className={cn([
                    "rounded-md",
                    "ring-offset-card hover:ring-border ring-offset-2 hover:ring-1",
                    onImageClick ? "cursor-pointer" : "cursor-default",
                  ])}
                >
                  <img
                    src={src}
                    alt={image.alt}
                    loading="lazy"
                    draggable={false}
                    className="border-border bg-card h-16 max-w-40 rounded-md border object-cover"
                  />
                </button>
              );
            }}
          </AttachmentResource>
        ))}
      </div>
    </VaultAttachmentProvider>
  );
}
