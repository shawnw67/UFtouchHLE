# Fonts

## Liberation fonts

These are taken from the Liberation fonts release 2.1.5: https://github.com/liberationfonts/liberation-fonts/releases/tag/2.1.5

See `LICENSE.liberation` for the license details.

## Noto fonts

These are taken from the Noto Sans CJK release that was current as of 2023-01-28: https://github.com/googlefonts/noto-cjk/tree/main/Sans

Specifically these are "Region-specific Subset OTFs" for Japan.

`NotoSansSC-Regular.otf` and `NotoSansSC-Bold.otf` are the "Region-specific
Subset OTFs" for Simplified Chinese from the same repository (as of
2026-10-14), further subset with `pyftsubset` (fontTools) to ASCII, Latin-1,
general and CJK punctuation, full-width forms and the characters of GB2312.

See `LICENSE.noto` for the license details.
