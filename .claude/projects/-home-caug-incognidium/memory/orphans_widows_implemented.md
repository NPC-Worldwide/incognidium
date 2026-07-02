---
name: orphans-widows-implemented
description: CSS orphans and widows properties now parsed for pagination control
metadata:
  type: project
---

# Orphans and Widows Properties Implemented

CSS orphans and widows properties have been implemented for pagination control.

## Properties Added

- `orphans` - minimum number of lines that must be left at the bottom of a page/column
- `widows` - minimum number of lines that must be left at the top of a page/column

## Fields Added to ComputedStyle

- `orphans: u32` - defaults to 2 (CSS specification default)
- `widows: u32` - defaults to 2 (CSS specification default)

## Parsing Support

Both properties accept positive integer values. The minimum is clamped to 1.

Example:
```css
p {
  orphans: 3;
  widows: 3;
}
```

This ensures that at least 3 lines of a paragraph will be kept together
before or after a page/column break.

## Why

Orphans and widows control typography in paged media and multi-column
layouts. They prevent single lines of text from being left alone at the
top or bottom of a page/column, which looks unprofessional.

## Compatibility

These properties are widely supported in modern browsers for print
media and in multi-column layouts.
