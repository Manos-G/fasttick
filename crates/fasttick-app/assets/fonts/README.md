# Bundled fonts

| File | Font | Version | Licence | Source |
|---|---|---|---|---|
| Roboto-*.ttf | Roboto | 2.138 | Apache-2.0 (see LICENSE) | https://github.com/googlefonts/roboto/releases/tag/v2.138 |

FastTick does not use egui's default fonts, because one of them uses the
Ubuntu Font Licence, which is not OSI-approved.

For monospace text, FastTick loads a monospace font that is installed on the
system (for example DejaVu Sans Mono or Liberation Mono) at runtime. If it
finds none, it uses Roboto.
