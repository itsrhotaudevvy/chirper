# Chirper brand assets

Identity: one white speech-bubble mark with a chirp notch on a softened rainbow (the DESIGN.md §3.2
spectrum, dark column). Meaning: many networks, one conversation. No text in the app icon, no gloss.

## Files

| file | use |
|---|---|
| `icon.svg` | 1024×1024 master, full-bleed square (the OS masks it). Mark inside the central 66 % safe zone. |
| `icon-macos.svg` | Same art inside an 824 px macOS squircle body (corner ~185 px) with a soft drop shadow. macOS does not mask, so use this for the macOS `.icns`. |
| `icon-background.svg` | Android adaptive background layer, 108 dp viewport. Rainbow only. |
| `icon-foreground.svg` | Android adaptive foreground layer, 108 dp viewport. White mark (within the 66 dp safe zone). |
| `icon-monochrome.svg` | Mark only, 108 dp viewport, black on transparent. For Android 13 themed icons and iOS tinted icons (the system recolours it). |
| `favicon.svg` | Simplified 64-unit variant for 16–32 px: four colour blobs, mark at 92 %. |
| `tray-template.svg` | macOS menu-bar template, 22 pt grid, black mark on transparent (macOS tints it). Name it `*Template` when shipping. |
| `tray.svg` | White mark on transparent, for dark Windows/Linux trays. |
| `tray-unread.svg` | Black template mark with a dot badge at top-right (unread). Still template-style, so it is monochrome. |
| `splash.svg` | 1024×1024 splash: white mark centred on `#0E1517` with a faint rainbow bloom. |
| `wordmark.svg` | Lowercase `chirper` wordmark, geometric strokes converted to paths (no font dependency). Colour `#E9EFF0` (dark-theme `--on`). |
| `png/icon-1024.png` | Rendered `icon.svg`. Input for `cargo tauri icon`. |
| `png/icon-macos-1024.png` | Rendered `icon-macos.svg`. Input for the macOS icon set. |
| `png/icon-64.png` | Rendered `icon.svg` at 64 px (check). |
| `png/icon-16.png` | Rendered `favicon.svg` at 16 px (check). |
| `png/favicon-32.png` | Rendered `favicon.svg` at 32 px (check). |
| `png/splash-1024.png` | Rendered `splash.svg`. |

## Colours

- Spectrum (rainbow blobs, 14 evenly spaced OKLCH hues, DESIGN.md §3.2 dark column), in ring order:
  `#FD7277 #F87C3C #DE9216 #BEA412 #91B519 #46C165 #0DBF9F #14BBC2 #11B5E4 #56A9FF #8F9AFE #BA87FB #DD7AD9 #F372AB`
- Base behind the rainbow: `#8A6FC9` (keeps the corners luminous, not near-black).
- Centre pool: `#070B14` at 34 % (centre) fading to 26 % at 60 % radius, behind the mark only. Edge vignette:
  `#070B14` at most 12 % at the corners. Measured white-on-background contrast under the mark: icon.svg 1024 min 3.99:1, favicon.svg min 4.61:1.
- Mark: `#FFFFFF`, with a black drop shadow at 30 % opacity.
- Splash background: `--bg` `#0E1517`.
- Wordmark: `#E9EFF0`.
- Tray and monochrome: pure black or white, so the OS can recolour them.

The old Aurora gradient (DESIGN.md §1) and the teal Openchirp bubble are retired.

## Generating platform icons

From the repo root, after `cargo tauri` is installed:

```sh
cargo tauri icon assets/brand/png/icon-1024.png
```

macOS must use the squircle art, which does not come from the 1024 master. Generate it separately:

```sh
cargo tauri icon assets/brand/png/icon-macos-1024.png --output <scratch-dir>
# then take icon.icns from <scratch-dir> into the macOS bundle icon path
```

Tray and favicon sizes are not produced by `cargo tauri icon`. Use the SVGs or
`png/favicon-32.png` / `png/icon-16.png` directly.

## Regenerating previews

Renderer: `rsvg-convert` (librsvg). Commands:

```sh
cd assets/brand
rsvg-convert -w 1024 -h 1024 icon.svg -o png/icon-1024.png
rsvg-convert -w 1024 -h 1024 icon-macos.svg -o png/icon-macos-1024.png
rsvg-convert -w 64 -h 64 icon.svg -o png/icon-64.png
rsvg-convert -w 16 -h 16 favicon.svg -o png/icon-16.png
rsvg-convert -w 32 -h 32 favicon.svg -o png/favicon-32.png
rsvg-convert -w 1024 -h 1024 splash.svg -o png/splash-1024.png
```
