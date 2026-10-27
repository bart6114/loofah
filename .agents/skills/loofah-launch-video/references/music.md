# Music for Loofah launch films

Research reviewed 2026-10-27. This reference combines the sources below with Loofah-specific creative recommendations. Numerical settings are starting points, not platform standards or a substitute for listening.

## Choose the cue

Include music in the normal launch-film export while keeping the message understandable with sound off. For Loofah, start with warm instrumental music: electronic or organic textures, a clear pulse, and an uplifting build. Match energy to the brief. A model announcement can benefit from a brighter, more spirited groove even with calm forest visuals; do not default to meditative ambient music simply because the palette is green. Avoid vocals, intrusive lead melodies, comic ukulele, aggressive drops, or trailer fanfare unless the particular brief calls for them. Roughly 90–120 BPM can be a useful launch starting point; the musical feel and edit matter more than the number.

Shortlist two or three plausible tracks and audition useful sections against the film, rather than choosing solely from tags or title. Listen for a readable opening, energy beneath the demonstration, and a phrase that can resolve beneath the final Loofah message. A busy cue may compete with reading even when there is no narration. Choose one track and explain the fit briefly; a routine music choice does not need an approval round.

Use actual music assets or an authorized composer/generation service when requested and available. Do not present placeholder oscillators, a metronome, or unlicensed demo music as a finished soundtrack. Do not imitate an artist on the assumption that it grants reuse rights.

## Source and preserve permission

Prefer a user-provided cleared cue or a primary artist/library source with clear permission for the intended commercial promotional distribution. Verify the exact track and applicable music license before committing to an edit. “Royalty-free” does not establish that a track is free, attribution-free, or allowed on every channel.

Practical options include a library with no required credit for online promotion or a suitable CC0/CC BY track from its verified creator. Follow any attribution conditions in the actual distribution: a private notes file alone does not supply public credit. Do not use a purchase, subscription, account creation, or uncertain licensing as an unannounced dependency.

Record title, artist, track page, download URL, license URL/name, retrieval date, attribution wording if required, selected source time range, and edit details in the project's asset notes. Save a local license snapshot or receipt where available. Retain the original asset locally for reproducibility, subject to its license. A stock music license may permit synchronization in the film while prohibiting public redistribution of the raw asset or Content ID registration. Loofah's MIT license does not relicense third-party music; keep such assets out of public source bundles when permission does not cover them, and provide the source/download manifest instead.

Example source, verified on the research date: Mixkit's [Stock Music Free License](https://mixkit.co/license/modal/musicFree/) permits web/social videos and online ads, with excluded media including broadcasts, games, and physical discs. Its [music FAQ](https://mixkit.co/free-stock-music/) says attribution is optional. Recheck the exact item and current terms for each use. This is an example source, not a required provider or an unrestricted license.

## Edit to the film

Map the chosen section to the existing story. Start on a useful phrase, support one or two major reveals with musical changes, and let the final identity/ownership message settle. Do not recut every readable title to every beat. For a music-only revision, preserve approved visuals unless a small timing change materially improves the result.

Choose a deliberate 30-second excerpt rather than the first 30 seconds automatically. Prefer an ending cadence or a clean phrase boundary. If extending a cue is permitted, loop at matching musical boundaries with a short crossfade and check for a seam; never assume any license allows a music-only remix. Avoid unnecessary speed/pitch changes. A short fade-in around 0.3–0.8 seconds and a gentle 1–2 second fade-out are useful initial choices; a natural composed ending may work better. Keep music under the ending message and resolve before the video finishes. Eliminate clicks and abrupt cuts.

## Mix and implement

For a music-led web review export without narration, try a finished integrated level around **−18 to −16 LUFS**, with a **true-peak ceiling around −1.5 dBTP**. Use a quieter balance when it suits the cue. These are project recommendations, not universal social-platform delivery rules. Follow the destination's actual specification when given.

With narration, establish clear speech first, then reduce/automate music under it. Do not normalize voice and music independently to the same loudness. Use listening and final-mix measurements rather than a universal percentage slider setting. Check the finished AAC for overshoot; leave additional headroom or re-encode if needed.

In Remotion, use the audio component supported by the installed version. Current guidance recommends `Audio` from `@remotion/media`; derive trim, duration, fades, and automation from the frame clock. Keep one continuous bed above the scene sequences so it does not restart on each cut. Alternatively, preprocess a correctly timed, faded, normalized WAV and reference it once at unity gain. Avoid applying fades/normalization twice. The editable composition and final mux must use the same prepared mix.

Preserve the original cue and the exact preparation command/script. FFmpeg's `loudnorm` supports measured two-pass normalization; feed the first-pass measurements into the second pass rather than assuming a single gain reaches a target. Choose the excerpt and fade arrangement before measuring, explicitly set a compatible output rate such as 48 kHz, and measure the final encoded result too. Keep prepared files private when source redistribution is restricted.

When only music changes, it is reasonable to stream-copy approved H.264 frames into a new MP4 with the prepared AAC track. Keep an editable audio layer/render path in the source, explicitly map video and audio streams, and verify exact total duration instead of relying on `-shortest` to hide a short cue. This avoids spending another visual encode on unchanged footage.

## Verify the delivered file

- Confirm an audio stream actually exists: AAC, expected channels/sample rate, and a duration covering the intended film. A source-code audio component is not evidence that the MP4 has sound.
- Decode both streams completely. Check non-silent audio, integrated loudness, true peak, unintended gaps, and the fades at the beginning/end. Example measurement: `ffmpeg -i out/film.mp4 -map 0:a:0 -af loudnorm=I=-18:TP=-1.5:LRA=11:print_format=json -f null -`; its **input** fields describe the existing encoded mix.
- Audition the exported file at an ordinary listening level, ideally on headphones and laptop speakers. Check the opening, transitions, final message, and last second. Metrics alone cannot establish that a track sounds appropriate. If the environment cannot hear playback, report that limit and do not claim an audible review from metadata alone.
- Keep the prior approved version and label the new music export. Deliver the MP4, editable audio path, and relevant credit/license notes. A separate silent version is useful when requested, not a replacement for the music export.

## Primary sources

- [Wistia: choosing background music](https://wistia.com/blog/choosing-music-for-your-video): match the track to the message, edit/loop suitable sections, and keep music from competing with speech. Applied here: audition against the story and use restrained arrangement and balance.
- [Remotion: audio](https://www.remotion.dev/docs/media/audio), [volume](https://www.remotion.dev/docs/audio/volume): local assets, trimming, duration, and frame-driven gain/fades. Check the installed API before implementation.
- [FFmpeg: loudnorm](https://ffmpeg.org/ffmpeg-filters.html#loudnorm): integrated loudness/true-peak targets, two-pass measurements, and sample-rate handling. The suggested Loofah mix level is a production choice, not FFmpeg's default.
- [Mixkit music license](https://mixkit.co/license/modal/musicFree/) and [FAQ](https://mixkit.co/free-stock-music/): source-specific online promotional permissions and attribution conditions.
