# waltcut-media

Media sources, still-image and image-sequence import, generated media, audio streams, and decoded
frame caching for FilmCraft.

## Import allowlist

The media browser exposes supported stills, audio and video sources only. The current video
containers are MP4/MOV, Matroska/WebM, AVI and MPEG-TS/PS; video includes H.264, HEVC, ProRes,
VP9, AV1 and MPEG-1/2. Supported audio includes PCM/WAV/AIFF, AAC, MP3/MP2 and Opus. Formats
removed from the allowlist return an actionable unsupported-format error even when the input is
empty or truncated. AC-3/E-AC-3 inside an otherwise-retained video import is the exception:
picture imports without that audio and the engine reports the conversion notice to the user.

## Image-sequence filenames

`sequence::Numbered` identifies the last run of 1–18 ASCII frame-number digits in a filename's
stem when the extension is a recognized still-image format. `/` and `\` separate directory
components. A prefix may contain other UTF-8 characters, including accented letters, CJK and emoji:
`é0001.png`, `图0001.png` and `🎞️0001.png`. Parsing preserves the prefix, number padding and suffix
when generating sibling-frame paths. Sibling recognition compares the prefix's UTF-8 bytes exactly.
Stems without ASCII digits or with a final frame-number run longer than 18 characters are not numbered
image-sequence filenames.
