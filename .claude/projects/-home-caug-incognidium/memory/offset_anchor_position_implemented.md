---
name: offset-anchor-position-implemented
description: CSS offset-anchor and enhanced offset-position for motion path
metadata:
  type: project
---

# Offset Anchor and Position Implemented

CSS motion path properties `offset-anchor` and `offset-position` have been implemented.

## Properties Added

- `offset-anchor` - defines the anchor point of the element on its motion path
- Enhanced `offset-position` - now supports position values and keywords

## Fields in ComputedStyle

- `offset_anchor: (f32, f32)` - normalized anchor point (x, y), default (0.5, 0.5)
- `offset_position: OffsetPosition` - position on the motion path

## Parsing Support

### offset-anchor
- `auto` - default center point (0.5, 0.5)
- `<position>` - two values like `50% 50%` or `left top`

### offset-position
- `auto` - follow normal flow
- `normal` - same as auto
- `<position>` - specific position on path
- Single value - used for both x and y

Example:
```css
.animated {
  offset-path: path("M 0,0 L 100,100");
  offset-anchor: center;
  offset-position: 50% 50%;
  offset-distance: 100px;
  offset-rotate: auto;
}
```

## Why

These properties complete the CSS Motion Path module, allowing elements to be
positioned along a path with control over the anchor point (where the element
attaches to the path) and position (where along the path the element appears).

## Related Properties

- `offset-path` - defines the motion path (url, path, none)
- `offset-distance` - distance along the path
- `offset-rotate` - rotation of the element as it follows the path
