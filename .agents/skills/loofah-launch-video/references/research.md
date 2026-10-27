# Research trail

Reviewed 2026-10-27. Primary sources below inform the workflow. Loofah-specific creative choices are a synthesis, not an official campaign or a claim that fixed timing guarantees performance.

## Launch-video direction

- [Wistia: Product Videos that Pop](https://wistia.com/blog/software-product-video-tips). Distinguishes overview, demo, and launch videos; emphasizes brand-appropriate creative ideas and pacing that lets a useful moment register. Applied here: an expressive opening followed by readable proof, with pauses around the payoff.
- [Wistia: Product page video practices](https://wistia.com/blog/product-video-best-practices). Discusses matching video type to the viewer's question, introducing the product early, and deliberate visual/music choices. Applied here: choose one viewer need, show actual work, and support the intended feeling with audio.
- [Google Ads: ABCDs of effective video ads](https://support.google.com/google-ads/answer/14783551?hl=en). Recommends an immediate story, early brand presence, focused messaging, and a clear next action. Applied here: recognizable identity in the opening, one takeaway, and one CTA. This advertising guidance does not establish a universal edit speed for an organic Loofah film. Its performance statistics are not Loofah evidence.

## Astra instructions

[OpenAI: GPT-6 prompting best practices](https://developers.openai.com/api/docs/guides/latest-model#prompting-best-practices), including the live [Astra guide](https://developers.openai.com/api/docs/guides/latest-model/gpt-6-astra.md), describes Astra's tendency to clarify, sensitivity to skill instructions, writing habits, and the need to calibrate verification.

Applied here: specify artifact, audience, source context, and observable completion criteria; make routine assumptions explicit; continue authorized production; load references conditionally; avoid unnecessary approval gates; inspect concrete output. Use with Astra in Codex/ChatGPT where suitable tools exist. Tool availability determines rendering capability.

## Reliable code-rendered video

- [Remotion: Animating properties](https://www.remotion.dev/docs/animating-properties) and [flickering](https://www.remotion.dev/docs/flickering) explain frame-driven animation, deterministic rendering, and asset readiness. Applied here: derive movement from frames and await media.
- [Remotion: Fonts](https://www.remotion.dev/docs/fonts) covers loading real fonts and waiting before text measurement. Applied here: preserve Fraunces and inspect exported typography.
- [Remotion: Render](https://www.remotion.dev/docs/cli/render) and [still](https://www.remotion.dev/docs/cli/still) support video and frame exports. Applied here: inspect stills before the final render, then review the sequence.

## Loofah evidence

[Homepage](https://loofah.io/), [artwork](https://loofah.io/artwork), [AI setup](https://loofah.io/ai-setup), and [offline behavior](https://loofah.io/offline), alongside repository paths in [brand.md](brand.md). The screenshot, forest poster, and canonical icon were visually inspected. CSS/theme source supplied exact typography and colors. The website product illustration and optional themes were kept distinct from the default app appearance.

Refresh product claims, styling, channel requirements, and rendering APIs when relevant to a new film. Preserve the research date rather than presenting this snapshot as current forever.

## Music update

The initial test export was silent despite being a launch film. The workflow now distinguishes sound-off legibility from a silent deliverable and includes licensed instrumental music by default. [music.md](music.md) contains the source trail and practical selection, editing, loudness, export, and listening checks, based on Wistia, Remotion, FFmpeg, and the source library's own license. It also distinguishes a measured technical check from an audible review.

## Art direction update

The user requested the character and craft of an independent animation studio. [art-direction.md](art-direction.md) adds a visual concept, recurring forms, transformations between scenes, expressive typography, varied motion weight, and integrated sound. This addresses the test film's reliance on repeated cards and rise/fade entrances. Primary process references are [Ordinary Folk](https://www.ordinaryfolk.co/process) and [Cub Studio](https://cubstudio.squarespace.com/animation-blog/animation-process); Loofah-specific visual suggestions are creative recommendations, not claims from those studios.
