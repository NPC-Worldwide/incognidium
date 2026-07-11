---
name: font-variation-settings-implemented
description: CSS font-variation-settings now parsed for variable font axis control
metadata:
  type: project
---

# Font Variation Settings Implemented

CSS `font-variation-settings` property has been implemented for controlling variable font axes.

## Property Added

- `font-variation-settings` - controls low-level variable font features

## Field in ComputedStyle

- `font_variation_settings: Vec<(String, f32)>` - list of axis tag and value pairs

## Parsing Support

The property accepts:
- `"normal"` - clears all variation settings
- `"wght" 700, "wdth" 200` - comma-separated axis/value pairs
- Single axis tag like `"wght"` (defaults to value 1.0)

Example:
```css
body {
  font-variation-settings: "wght" 700, "wdth" 200;
}
```

## Why

Variable fonts allow a single font file to contain multiple design variations
defined by axes like weight (wght), width (wdth), slant (slnt), etc. The
font-variation-settings property provides low-level access to these axes for
fine-grained typographic control.

## Compatibility

This property works with variable fonts that support OpenType variation axes.
Not all fonts support all axes - common axes include wght, wdth, slnt, opsz.
