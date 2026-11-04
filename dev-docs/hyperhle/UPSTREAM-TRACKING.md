# HyperHLE-Reborn reference tracking

Latest reference checked on 2026-11-04:
`371f90ce65073c62cf216623d946a4daa399ab32` (`hyper-audit/trunk`).
Previous reference: `1d71f7c2881c2b0f58c8a2831bce0d731f93c73c`.

The new range contains three commits and changes four paths:
`src/environment/app_picker.rs`, `OPTIONS_HELP.txt`,
`touchHLE_options.txt`, and `CHANGELOG.md`.
It adds rotation/movie-player compatibility switches to the picker, with
explicit on/off overrides that preserve per-app defaults when untouched.
These switches remain pending: UF does not yet implement the corresponding
`rotation_fix` / `fix_movie_player` options and runtime behavior. Port the
runtime behavior before exposing controls in either UF launcher.

The reference was fetched and advanced for comparison only. It was not merged
into the port branch. `unique-declarations.json` remains the original fixed
declaration snapshot; its coverage audit is a textual comparison, not a measure
of game compatibility or of every change in the latest reference.

Recent AudioConverter work implements packed interleaved and planar mono/stereo LPCM at
matching sample rates, integer/float and endian conversion, buffer-list and
input-callback APIs. Unsupported formats fail explicitly. Sample-rate
conversion and compressed codecs remain pending. Real-game and
Android-device verification are deferred at the user's request.
