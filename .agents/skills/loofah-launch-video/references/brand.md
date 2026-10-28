# Loofah visual and product reference

Observed 2026-10-27 in checkout `f496c3ad4d`, the live [homepage](https://loofah.io/), and the assets below. Refresh relevant sources for each film. Paths are relative to the Loofah repository root.

## Marketing identity

The homepage combines sunlit forest leaves, dark green framing, warm pale text, rounded product framing, and soft expressive serif headlines. The feeling is calm, tactile, independent, and useful. Let the film gain energy from meaningful editing and product reveals.

Sources: `docs/src/styles/custom.css` (`.home-page`, `.home-hero`, `.product-demo`), `docs/src/pages/index.astro`, and `docs/src/components/HomeSections.astro`.

| Role | Homepage token |
| --- | --- |
| Main background | `#142519` |
| Main text | `#f8faf0` |
| Leaf emphasis / CTA fill | `#d7edaa` |
| Supporting copy | `#e0e7db` |
| Muted copy | `#d5dfd2` |
| CTA ink | `#193c2c` |
| Quiet border | `#758574` |
| Product frame | `#e6e9e1`, border `#bcc8bf` |

General docs tokens also include mint `#7ad6aa` and light background `#fffdf7`; distinguish these from the homepage's primary forest/leaf treatment.

Headlines: **Fraunces Variable**, weight 650, axes `opsz 72`, `SOFT 65`, `WONK 1`; homepage tracking `-0.06em`, line height `0.92`. Italic emphasis uses weight 520 and `SOFT 90`. Brand text uses weight 650, `opsz 28`, `SOFT 80`, tracking `-0.02em`. Adjust tracking and line height for readability in motion rather than blindly copying web values.

Supporting copy uses the site's system sans treatment; small proof labels use system monospace. The website product illustration specifies Inter with system fallbacks. Load actual fonts before measuring/rendering. The docs depend on `@fontsource-variable/fraunces`; reuse available licensed font files rather than accepting an unnoticed Georgia fallback.

## Canonical assets

| Asset | Repository path / use |
| --- | --- |
| Full app icon | `apps/desktop/src-tauri/icons/src/loofah-squircle-1024.png` |
| Standalone mark | `apps/desktop/src-tauri/icons/src/loofah-mark-1024.png` — inspect alpha/foreground before placing |
| Product screenshot | `docs/public/screenshots/loofah-primary.png` — synthetic “Autumn launch planning” session |
| Nature still | `docs/public/images/forest-leaves.jpg` — current homepage poster |
| Nature motion | `docs/public/videos/forest-leaves.mp4` — current homepage backdrop |
| Alternative backdrop | `docs/public/images/loofah-hero-backdrop.jpg` — inspect before use; not the current homepage poster |

The full icon has a pale cream/sage squircle and dark botanical cross-section mark. Reuse it; do not redraw it or substitute a generic leaf/microphone. Preserve proportions and transparency. Marketing uses “Loofah”; the app sidebar may use lowercase “loofah”.

The homepage credits the forest clip to this [Pexels source](https://www.pexels.com/video/serene-sunlight-through-lush-forest-leaves-31883946/). Retain provenance when reusing it and check the relevant license for a new use. Keep nature footage subdued beneath text. The homepage uses dark directional overlays, strongest behind copy.

Public fallbacks: [artwork gallery](https://loofah.io/artwork), [screenshot](https://loofah.io/screenshots/loofah-primary.png). If the checkout is unavailable, use public references and record which assets are captures or original illustrations in project notes.

## Product appearance

The screenshot is spacious and document-focused: narrow session sidebar, broad white editor, Summary / Note / Transcript tabs, muted green selection, restrained borders, and compact controls. Crop to the result; shrinking the entire screenshot until its text is unreadable loses the proof.

The desktop source of truth is `apps/desktop/src/shared/theme/catalog.ts`, with rules in `apps/desktop/src/styles/themes.css` and base fonts in `apps/desktop/src/styles/globals.css`.

The **Default** theme is neutral: light paper `#ffffff`, ink `#1c1c1c`, shell/sidebar `#f5f5f5`, accent `#292929`; it uses macOS system sans. Other selectable themes have their own palettes and typography. **Workshop**, for example, uses paper `#fcfcf5`, forest accent `#38654d`, Literata headings, system sans body, and IBM Plex Mono details. It is optional, not the universal default.

Use website styling around the product and preserve the captured theme inside it. Do not recolor controls or replace app fonts with Fraunces to invent a matching UI. Keep readable UI shots front-facing; a slight angle can serve an establishing shot but should settle for the proof.

## Message and claims

Current homepage lead: “Free meeting transcription. Notes you own.” Supported themes include recording without a meeting bot, transcription on the Mac, Markdown notes in a local vault, optional AI summaries, and agent access. Choose the subset that matters to the launch.

Verify specifics in `docs/src/content/docs/{ai-setup,offline,data-and-privacy,languages,quickstart}.mdx` and current implementation before putting them on screen.

- Recording and transcription run locally. Offline transcription needs a previously downloaded speech model.
- Optional Intelligence for summaries/titles uses a configured local model or hosted provider with the user's API key. Hosted usage may cost money and sends needed context to that provider. Avoid unconditional claims that all AI is offline or nothing ever leaves the Mac.
- Live transcription and speaker detection have different model/language conditions. Current docs describe live text for supported English meetings and speaker detection for imported/after-recording transcripts. Do not imply live speaker identification.
- Current desktop distribution is Apple Silicon, macOS 15+. Verify before export. Do not add platforms based on stray work-in-progress files.
- The app is free, MIT open source, with no Loofah account/subscription. Optional third-party provider costs are separate.
- Markdown notes and vault files establish ownership/portability. Do not imply a proprietary cloud database or require an account in a demo.

The screenshot's November beta text is synthetic meeting content, not a Loofah release date.

## Nemotron announcement direction

For the Nemotron launch film, the approved announcement is that **Nemotron 3.5 ASR** has joined Loofah's offline transcription models. Preserve the **ASR** suffix in visible titles, model labels, and supporting copy; a shorter internal enum/display label is not a reason to drop it from the campaign name. Use English copy and example text. Demonstrate speech becoming words with a descriptive sample such as “Every spoken word, written down.” rather than an apparent request to discuss next steps. Remove on-screen production disclaimers. The closing explains Loofah: “All your meetings transcribed. Fully locally. Own your data.” with a clear “Free and open source” line and `loofah.io`. Keep the local claim tied to transcription; optional hosted summaries remain a separate choice.

These are campaign-specific decisions to retain when revising this film, not a mandatory script for every feature launch. Its soundtrack should feel positive and spirited; quiet forest visuals do not require a slow meditative cue.
