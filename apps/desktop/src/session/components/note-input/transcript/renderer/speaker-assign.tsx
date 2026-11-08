import { Plural } from "@lingui/react/macro";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { Checkbox } from "@hypr/ui/components/ui/checkbox";
import {
  Popover,
  PopoverAnchor,
  PopoverContent,
} from "@hypr/ui/components/ui/popover";
import { cn } from "@hypr/utils";

import { ensurePerson, type Person } from "~/people/queries";
import {
  legacyTranscriptControlsAvailable,
  useVaultReadScope,
} from "~/shared/application/use-vault-read-scope";
import type { Segment } from "~/stt/live-segment";
import { assignTranscriptSpeaker, type TranscriptRecord } from "~/stt/queries";

export type ChannelAssignmentState = {
  anchorWordIdBySpeakerIndex: Map<number, string>;
  channelHasAssignment: boolean;
};

export const EMPTY_CHANNEL_ASSIGNMENT_STATE: ChannelAssignmentState = {
  anchorWordIdBySpeakerIndex: new Map(),
  channelHasAssignment: false,
};

export function SpeakerRenameControl({
  segment,
  sessionId,
  transcriptId,
  color,
  label,
  className,
  disabled,
  onAssigned,
  people = [],
  channelAssignmentState = EMPTY_CHANNEL_ASSIGNMENT_STATE,
}: {
  segment: Segment;
  sessionId: string;
  transcriptId: string;
  color: string;
  label: string;
  className?: string;
  disabled?: boolean;
  onAssigned?: (speakerLabel: string) => void;
  people?: readonly Person[];
  channelAssignmentState?: ChannelAssignmentState;
}) {
  const { scope, current } = useVaultReadScope();
  const target = useRef({ sessionId, transcriptId });
  target.current = { sessionId, transcriptId };
  const locked = useRef(disabled);
  locked.current = disabled;
  const active = useRef(false);
  useLayoutEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
    };
  }, []);
  const available = useCallback(
    () =>
      active.current &&
      current() &&
      !!sessionId &&
      target.current.sessionId === sessionId &&
      target.current.transcriptId === transcriptId &&
      !locked.current &&
      legacyTranscriptControlsAvailable(scope),
    [scope, current, sessionId, transcriptId],
  );
  const check = useCallback(() => {
    if (!available())
      throw new Error("Transcript editing is unavailable for this connection");
  }, [available]);
  const assign = useCallback(
    async (input: Parameters<typeof assignTranscriptSpeaker>[0]) => {
      check();
      await assignTranscriptSpeaker(input, check);
      check();
    },
    [check],
  );
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(label);
  const [touched, setTouched] = useState(false);
  const [highlightIndex, setHighlightIndex] = useState(-1);
  const [applyToChannel, setApplyToChannel] = useState(false);
  // Optimistic label: shown from commit until the store round trip updates the
  // label prop, so the rename feels instant on large transcripts.
  const [pendingLabel, setPendingLabel] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const committedRef = useRef(false);
  const commitLabelRef = useRef(label);
  // Distinguishes settles of superseded commits (rapid successive renames) from
  // the latest one: only the latest commit may clear or keep the pending label.
  const commitSeqRef = useRef(0);
  const commitSettledRef = useRef(true);
  const labelRef = useRef(label);
  labelRef.current = label;
  const { anchorWordIdBySpeakerIndex, channelHasAssignment } =
    channelAssignmentState;
  // A first assignment on a diarized channel most likely names the whole side
  // (one person per channel is the common case); once any assignment exists,
  // renames target only the clicked cluster.
  const offerChannelWideAssign =
    anchorWordIdBySpeakerIndex.size >= 2 && !channelHasAssignment;
  const otherClusterCount =
    segment.key.speaker_index != null &&
    anchorWordIdBySpeakerIndex.has(segment.key.speaker_index)
      ? anchorWordIdBySpeakerIndex.size - 1
      : anchorWordIdBySpeakerIndex.size;

  useEffect(() => {
    // Before the latest commit settles, a label change belongs to an OLDER
    // rename's round trip — the pending label (the user's newest intent) stays.
    if (
      pendingLabel !== null &&
      commitSettledRef.current &&
      label !== commitLabelRef.current
    ) {
      setPendingLabel(null);
    }
  }, [label, pendingLabel]);

  useEffect(() => {
    if (!editing) {
      setDraft(label);
    }
  }, [editing, label]);

  const suggestions = useMemo(() => {
    if (!editing) {
      return [];
    }
    // The prefilled draft is the current label, not a search query — only filter
    // once the user has typed.
    const query = touched ? draft.trim().toLowerCase() : "";
    return people
      .filter((person) => {
        if (person.name === label) {
          return false;
        }
        return !query || person.name.toLowerCase().includes(query);
      })
      .slice(0, 8);
  }, [draft, editing, label, people, touched]);

  useEffect(() => {
    setHighlightIndex(-1);
  }, [draft]);

  const startEditing = useCallback(() => {
    if (!available()) return;
    committedRef.current = false;
    setDraft(label);
    setTouched(false);
    setHighlightIndex(-1);
    setApplyToChannel(false);
    setEditing(true);
  }, [label, available]);

  const runAssignments = useCallback(
    async (speakerLabel: string) => {
      if (offerChannelWideAssign && applyToChannel) {
        // Channel-scope fallback first (only needed when the clicked segment
        // itself has no index): later per-index writes keep it on a diarized
        // channel, while the reverse order would let it evict them.
        if (segment.key.speaker_index == null) {
          const anchorWordId = getAssignmentAnchorWordId(segment);
          if (anchorWordId) {
            await assign({
              transcriptId,
              segmentKey: segment.key,
              speakerLabel,
              anchorWordId,
            });
          }
        }
        for (const [speakerIndex, anchorWordId] of anchorWordIdBySpeakerIndex) {
          await assign({
            transcriptId,
            segmentKey: {
              channel: segment.key.channel,
              speaker_index: speakerIndex,
              speaker_human_id: null,
            },
            speakerLabel,
            anchorWordId,
          });
        }
        return;
      }

      const anchorWordId = getAssignmentAnchorWordId(segment);
      if (!anchorWordId) {
        return;
      }

      await assign({
        transcriptId,
        segmentKey: segment.key,
        speakerLabel,
        anchorWordId,
      });
    },
    [
      assign,
      anchorWordIdBySpeakerIndex,
      applyToChannel,
      offerChannelWideAssign,
      segment,
      transcriptId,
    ],
  );

  const commit = useCallback(
    (
      optimisticName: string,
      run: () => Promise<{ id: string; name: string }>,
    ) => {
      if (committedRef.current || !available()) {
        return;
      }
      committedRef.current = true;
      const seq = ++commitSeqRef.current;
      commitLabelRef.current = label;
      commitSettledRef.current = false;
      setPendingLabel(optimisticName);
      setEditing(false);
      run()
        .then((person) => {
          check();
          onAssigned?.(person.id);
          if (seq !== commitSeqRef.current) {
            return;
          }
          commitSettledRef.current = true;
          // Two shapes where the label prop will never (or already did) move,
          // so waiting on the effect above would strand the pending label: the
          // rename resolved to the name already shown (ensurePerson reuses
          // people case-insensitively), or the prop updated before this settle.
          if (
            person.name === commitLabelRef.current ||
            labelRef.current !== commitLabelRef.current
          ) {
            setPendingLabel(null);
          }
        })
        .catch((error) => {
          console.error("[transcript] failed to assign speaker", error);
          if (seq !== commitSeqRef.current) {
            return;
          }
          commitSettledRef.current = true;
          setPendingLabel(null);
        });
    },
    [label, onAssigned, available, check],
  );

  const canAssign =
    Boolean(getAssignmentAnchorWordId(segment)) ||
    (offerChannelWideAssign && applyToChannel);

  const saveFreeText = useCallback(() => {
    if (committedRef.current) {
      return;
    }

    const name = draft.trim();
    if (!name || name === label || !canAssign) {
      committedRef.current = true;
      setEditing(false);
      return;
    }

    commit(name, async () => {
      check();
      const person = await ensurePerson(name);
      check();
      await runAssignments(person.id);
      return { id: person.id, name: person.name };
    });
  }, [canAssign, commit, draft, label, runAssignments, check]);

  const selectPerson = useCallback(
    (person: Person) => {
      if (!canAssign) {
        committedRef.current = true;
        setEditing(false);
        return;
      }

      commit(person.name, async () => {
        await runAssignments(person.id);
        return { id: person.id, name: person.name };
      });
    },
    [canAssign, commit, runAssignments],
  );

  if (editing && available()) {
    return (
      <Popover open={suggestions.length > 0 || offerChannelWideAssign}>
        <PopoverAnchor asChild>
          <input
            ref={inputRef}
            autoFocus
            type="text"
            value={draft}
            onChange={(e) => {
              setTouched(true);
              setDraft(e.target.value);
            }}
            onBlur={saveFreeText}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setHighlightIndex((index) =>
                  suggestions.length === 0
                    ? -1
                    : (index + 1) % suggestions.length,
                );
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setHighlightIndex((index) =>
                  suggestions.length === 0
                    ? -1
                    : (index <= 0 ? suggestions.length : index) - 1,
                );
              } else if (e.key === "Enter") {
                e.preventDefault();
                const highlighted = suggestions[highlightIndex];
                if (highlighted) {
                  selectPerson(highlighted);
                } else {
                  inputRef.current?.blur();
                }
              } else if (e.key === "Escape") {
                e.preventDefault();
                committedRef.current = true;
                setDraft(label);
                setEditing(false);
              }
            }}
            style={{ color }}
            className={cn([
              "-my-0.5 min-w-0 rounded-full bg-transparent py-0.5 pr-2",
              "border-none outline-hidden",
              className,
            ])}
          />
        </PopoverAnchor>
        <PopoverContent
          align="start"
          className="w-56 p-1"
          onOpenAutoFocus={(e) => e.preventDefault()}
        >
          {offerChannelWideAssign && (
            <label
              // Keep the input focused so toggling doesn't commit via blur.
              onPointerDown={(e) => e.preventDefault()}
              className={cn([
                "flex cursor-pointer items-center gap-2 px-2 py-1",
                "text-muted-foreground text-sm",
              ])}
            >
              <Checkbox
                checked={applyToChannel}
                onCheckedChange={(checked) =>
                  setApplyToChannel(checked === true)
                }
              />
              <Plural
                value={otherClusterCount}
                one="Also apply to the other speaker on this channel"
                other="Also apply to the other # speakers on this channel"
              />
            </label>
          )}
          <ul role="listbox">
            {suggestions.map((person, index) => (
              <li key={person.id}>
                <button
                  type="button"
                  role="option"
                  aria-selected={index === highlightIndex}
                  onPointerDown={(e) => e.preventDefault()}
                  onClick={() => selectPerson(person)}
                  onMouseEnter={() => setHighlightIndex(index)}
                  className={cn([
                    "w-full rounded-sm px-2 py-1 text-left text-sm",
                    index === highlightIndex && "bg-accent",
                  ])}
                >
                  {person.name}
                </button>
              </li>
            ))}
          </ul>
        </PopoverContent>
      </Popover>
    );
  }

  return (
    <button
      type="button"
      disabled={
        disabled || !current() || !legacyTranscriptControlsAvailable(scope)
      }
      onClick={startEditing}
      className={cn([
        "-my-0.5 rounded-full py-0.5 pr-2",
        disabled
          ? "cursor-default"
          : "cursor-pointer underline-offset-2 hover:underline focus-visible:underline",
        className,
      ])}
      style={{ color }}
    >
      {available() ? (pendingLabel ?? label) : label}
    </button>
  );
}

export function getAssignmentAnchorWordId(
  segment: Segment,
): string | undefined {
  const word = segment.words.find(
    (word) => typeof word.id === "string" && word.id.length > 0,
  );
  return typeof word?.id === "string" ? word.id : undefined;
}

export function buildChannelAssignmentStates(
  transcript: TranscriptRecord | null,
): Map<Segment["key"]["channel"], ChannelAssignmentState> {
  const states = new Map<Segment["key"]["channel"], ChannelAssignmentState>([
    ["DirectMic", createChannelAssignmentState()],
    ["RemoteParty", createChannelAssignmentState()],
    ["MixedCapture", createChannelAssignmentState()],
  ]);
  const channelByWordId = new Map<string, number>();
  for (const word of transcript?.words ?? []) {
    if (typeof word.id === "string" && word.id) {
      channelByWordId.set(
        word.id,
        typeof word.channel === "number" ? word.channel : 0,
      );
    }
  }

  const indexByWordId = new Map<string, number>();
  for (const hint of transcript?.speakerHints ?? []) {
    if (
      hint.type !== "provider_speaker_index" ||
      typeof hint.word_id !== "string"
    ) {
      continue;
    }

    const value = parseProviderSpeakerHintValue(hint.value);
    if (!value) {
      continue;
    }

    indexByWordId.set(hint.word_id, value.speaker_index);
    if (typeof value.channel === "number") {
      channelByWordId.set(hint.word_id, value.channel);
    }
  }

  for (const [wordId, speakerIndex] of indexByWordId) {
    const channel = channelName(channelByWordId.get(wordId));
    if (!channel) continue;
    const state = states.get(channel);
    if (state && !state.anchorWordIdBySpeakerIndex.has(speakerIndex)) {
      state.anchorWordIdBySpeakerIndex.set(speakerIndex, wordId);
    }
  }

  for (const hint of transcript?.speakerHints ?? []) {
    if (hint.type !== "speaker_label" || typeof hint.word_id !== "string") {
      continue;
    }
    const channel = channelName(channelByWordId.get(hint.word_id));
    if (!channel) continue;
    const state = states.get(channel);
    if (state) {
      state.channelHasAssignment = true;
    }
  }

  return states;
}

function createChannelAssignmentState(): ChannelAssignmentState {
  return {
    anchorWordIdBySpeakerIndex: new Map(),
    channelHasAssignment: false,
  };
}

function channelName(
  channel: number | undefined,
): Segment["key"]["channel"] | undefined {
  if (channel === 0) return "DirectMic";
  if (channel === 1) return "RemoteParty";
  if (channel === 2) return "MixedCapture";
  return undefined;
}

function parseProviderSpeakerHintValue(
  value: unknown,
): { speaker_index: number; channel?: number } | null {
  let parsed = value;
  if (typeof parsed === "string") {
    try {
      parsed = JSON.parse(parsed);
    } catch {
      return null;
    }
  }

  if (!parsed || typeof parsed !== "object") {
    return null;
  }

  const speakerIndex = (parsed as { speaker_index?: unknown }).speaker_index;
  if (typeof speakerIndex !== "number") {
    return null;
  }

  const channel = (parsed as { channel?: unknown }).channel;
  return {
    speaker_index: speakerIndex,
    ...(typeof channel === "number" ? { channel } : {}),
  };
}
