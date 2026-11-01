import { useMemo, useState } from "react";

import { time, type Session } from "./api";

export function groupTranscript(words: Session["transcript"]) {
  const paragraphs: { start: number; text: string }[] = [];
  let text = "";
  let start = 0;
  let end = 0;
  let count = 0;
  for (const word of words) {
    const next = word.text.trim();
    if (!next) continue;
    if (
      text &&
      (word.start - end >= 1.5 ||
        text.length + next.length > 700 ||
        count >= 100 ||
        (text.length >= 300 && /[.!?]["')\]]?$/.test(text)))
    ) {
      paragraphs.push({ start, text });
      text = "";
      count = 0;
    }
    if (!text) start = word.start;
    const separator =
      text && !/^[,.;:!?%)\]}]/.test(next) && !/[(\[{]$/.test(text) ? " " : "";
    text += separator + next;
    end = word.end;
    count += 1;
  }
  if (text) paragraphs.push({ start, text });
  return paragraphs;
}

export function Transcript({
  words,
  onSeek,
}: {
  words: Session["transcript"];
  onSeek?: (seconds: number) => void;
}) {
  const paragraphs = useMemo(() => groupTranscript(words), [words]);
  const [visible, setVisible] = useState(40);
  return (
    <div className="transcript">
      {paragraphs.slice(0, visible).map((paragraph, index) => (
        <p key={`${paragraph.start}-${index}`}>
          {onSeek ? (
            <button
              className="transcript-timestamp"
              aria-label={`Play from ${time(paragraph.start)}`}
              onClick={() => onSeek(paragraph.start)}
            >
              {time(paragraph.start)}
            </button>
          ) : (
            <small>{time(paragraph.start)}</small>
          )}
          <span>{paragraph.text}</span>
        </p>
      ))}
      {paragraphs.length > visible && (
        <button onClick={() => setVisible((count) => count + 40)}>
          Show more transcript
        </button>
      )}
    </div>
  );
}
