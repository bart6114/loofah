# Production and inspection

Use the user's existing editor/pipeline when specified. These recommendations cover code-rendered product videos; verify installed versions and current official APIs before implementation.

## Assets and direction

Keep assets and a small provenance inventory with the video project. Record origins of captures, fonts, music, and nature footage. Use real product text or synthetic demo content consistently. Capture enough resolution for the intended crop; do not upscale a tiny UI excerpt to fill the screen.

Start with a strong composition. Give titles generous negative space and reserve the brightest leaf color for emphasis and CTA. Keep nature footage behind copy dark enough to read. Avoid generic neon gradients, glass panels, floating dashboards, and particles unless the specific concept calls for them.

At 1080p, try 72–110 px titles and 30–42 px supporting text, then inspect the export at realistic viewing size. These are starting points. Keep crucial content within a generous margin and check the actual channel's current overlay/safe-area requirements for social exports.

Build motion around the concept in [art-direction.md](art-direction.md). Use motivated anticipation, distinct object weight, shared forms across scenes, and a few well-developed transformations. Overshoot, arcs, masks, perspective, and secondary motion are useful when they add character. Do not default every element to the same small translation/fade. Keep the main focus clear, and let text or UI settle before it must be read. Use deliberate bursts and holds instead of uniformly fast movement.

Retain enough context when highlighting a detail. Cursors should move purposefully, click real controls, then get out of the way. Actual captures provide interaction proof; composited highlights are editorial annotations, not new app behavior.

## Remotion

- Confirm versions and composition conventions. Keep related Remotion packages compatible; record dependencies in the lockfile.
- Drive animation from `useCurrentFrame()` with timing derived from `useVideoConfig().fps`. Use `interpolate()` or `spring()` as appropriate. Frames must render correctly out of order.
- Avoid CSS keyframes/transitions, wall-clock timers, and uncontrolled randomness in exported animation. Convert website/app motion to the renderer's frame clock. The app's `motion/react` convention does not make wall-clock animation deterministic in Remotion.
- Use render-aware media components appropriate to the installed version. Await fonts and asynchronous assets so exports contain no loading states or fallback fonts.
- Keep timing and copy easy to edit. Check sequence-local versus global frames and transition overlaps; final duration must include all intended content.
- Bundle local assets where practical to avoid changing remote dependencies during rendering.

Official references: [animation](https://www.remotion.dev/docs/animating-properties), [flicker/loading](https://www.remotion.dev/docs/flickering), [fonts](https://www.remotion.dev/docs/fonts). Consult current [agent guidance](https://www.remotion.dev/docs/ai/skills) for deeper help; installing another skill is optional.

## Audio and export

Use [music.md](music.md) for music selection, licensing, timing, mixing, and audio QA. Include instrumental music by default unless the brief specifies silence. Keep sound-off viewing coherent as well.

If narration is requested, write to actual timing and test aloud. Roughly two spoken words per second is a planning starting point, with room for pauses. Time subtitles to final audio. Keep music below speech; avoid a sound effect on every cut. Use authorized voices and traceable audio assets.

Default review/share export: MP4, H.264, `yuv420p`, 30 fps, stereo AAC audio at 48 kHz when music/speech is included. Adapt to destination or existing pipeline. For narration, provide SRT/VTT and burn captions in when distribution needs it. Create a poster, cutdowns, and alternate ratios according to the brief.

Typical commands after dependencies and entry point exist:

```sh
pnpm exec remotion still src/index.ts LoofahLaunch out/proof.png --frame=360
pnpm exec remotion render src/index.ts LoofahLaunch out/loofah-launch.mp4 --codec=h264 --pixel-format=yuv420p
ffprobe -v error -show_entries format=duration:stream=codec_name,width,height,avg_frame_rate,pix_fmt -of json out/loofah-launch.mp4
```

Use actual entry point/composition names and confirm flags against installed versions. [Still CLI](https://www.remotion.dev/docs/cli/still), [render CLI](https://www.remotion.dev/docs/cli/render). Metadata confirms technical properties, not film quality.

## Definition of done

Render and inspect the opening, stable state of each proof, intermediate transition poses, boundaries likely to clip/flash, and closing card. View a contact sheet or stills at output resolution. Check line breaks, font identity, overflow, small UI, contrast, aspect ratio, cursor position, framing, and the continuity of forms. Use a short motion proof to judge important transitions before the full render; it should reveal the intended choreography, not just confirm that the code compiles.

Watch the full export in sequence using available playback tools. Review once without sound; if sound exists, review sync, level, and ending too. Stills cannot establish pacing. If playback is unavailable, inspect evenly spaced and boundary frames, report that limitation, and do not claim a full playback review.

Verify duration, dimensions, frame rate, video/audio codecs, and successful decoding. For a soundtrack export, confirm a non-silent audio stream, correct duration, and clean beginning/ending; measure the encoded mix and listen when available. Check that the CTA stays visible and the last frame is intentional. Run the video project's typecheck/build and required formatter when source changed. Re-render after a material correction.

Deliver the video, source location, exact render command, and relevant captions/credits. Report actual verification and missing capabilities. Local rendering does not authorize publishing or uploading.
