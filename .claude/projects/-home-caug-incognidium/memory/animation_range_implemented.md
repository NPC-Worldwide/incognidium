---
name: animation-range-implemented
description: CSS animation-range properties now parsed for scroll-driven animations
metadata:
  type: project
---

# Animation Range Properties Implemented

CSS animation-range properties have been implemented for scroll-driven animations:

## Properties Added

- `animation-range` - shorthand for range start and end
- `animation-range-start` - start offset of animation range
- `animation-range-end` - end offset of animation range

## Types Added

- `AnimationRange` enum with variants:
  - `Normal` - default range (entry 0% to exit 100%)
  - `Fixed(start, end)` - custom range with start and end offsets

- `AnimationRangeOffset` enum with variants:
  - `Named(name)` - named range values: "normal", "entry", "exit", "cover", "contain"
  - `Percentage(p)` - percentage offset value
  - `NamedWithPercentage(name, p)` - named range with percentage: "entry 0%"

## Parsing Support

The `animation-range` shorthand supports:
- Single keyword: `animation-range: normal`
- Two values: `animation-range: entry 0% exit 100%`
- Named ranges with percentages: `animation-range: cover 20%`

## Fields Added to ComputedStyle

- `animation_range: AnimationRange`
- `animation_range_start: AnimationRangeOffset`
- `animation_range_end: AnimationRangeOffset`

## Why

Scroll-driven animations require precise control over when an animation
starts and ends relative to the scroll position. The animation-range
properties allow authors to define these ranges using named timeline
ranges (entry, exit, cover, contain) combined with percentage offsets.

## How to Apply

These properties work with `animation-timeline: scroll()` or
`animation-timeline: view()` to control when animations are active
relative to the scroll/view timeline progress.
