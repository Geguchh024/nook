# Nook view components

Source of truth: `crates/modules/src/plugins/protocol.rs` (fields) and `view.rs` (drawing, events). A view is a tree of nodes; every node has a `type` and may have a `tooltip` (string) and `drag` (array of file paths: dragging the node drags those files out of Nook, as from Explorer; controls inside still click, the drag starts once the pointer moves, and a `dragged` message follows). Send the whole tree each time. Python: `nook.draggable(node, paths)`.

**Required** fields are in bold. Unknown fields are ignored; an unknown `type` draws nothing (so newer plugins still work on older Nook). A missing required field or a wrong JSON type (e.g. `"value": "0.5"` instead of `0.5`) makes the *whole message* invalid and it is skipped.

## Contents
1. Space available
2. Layout nodes
3. Content nodes
4. Controls
5. Canvas and shapes
6. Colors, text sizes, icons
7. Patterns

## 1. Space available

| Target | Content size (points), default | Fits |
|---|---|---|
| small tile (1×1) | 44 × 44 | one icon, a ring, or 1–3 characters |
| wide tile (2×1) | 104 × 36 | icon + two short lines, or a number + label |
| large tile (2×2) | 104 × 104 | a small chart, a ring with text, 3–4 short rows |
| page | 536 × 168 (or `page_size` minus header and padding) | lists, inputs, cards, settings |

The real numbers arrive in `layout` messages; the user can resize tiles. Text that is too long ends in "…" unless `wrap: true`.

## 2. Layout nodes

| type | Fields (default) | Notes |
|---|---|---|
| `column` | `children` ([]), `gap` (6), `align` (`start`/`center`/`end`, start), `justify` (start), `scroll` (false), `id` | Top to bottom. `align` is horizontal; `justify` places the whole stack vertically in the available height. `scroll: true` for page content that may overflow; with `justify: end` it opens at the bottom and follows new content (a chat). `id` keys the scroll position: a new id starts a fresh scroll. |
| `row` | `children` ([]), `gap` (6), `align` (start) | Left to right. `align` is vertical; use `"center"` for icon + text rows. |
| `grid` | **`columns`** (int ≥ 1), `children`, `gap` (6) | Equal-width cells, filled row by row. |
| `card` | `children`, `padding` (10), `gap` | Glass panel grouping its children (a column inside). |
| `spacer` | `size` | With `size`: a fixed gap. Without: pushes everything after it to the far end of the row/column (`[title, spacer, button]` = button on the right). |
| `divider` | | Thin line. |

## 3. Content nodes

| type | Fields (default) | Notes |
|---|---|---|
| `text` | **`text`** (string), `size` (`body`), `color` (`text`), `bold`, `wrap`, `mono` | `title`, `big` and `huge` are semibold automatically. `text` must be a **string**: send `"42"`, not `42` (the Python/Node helpers convert for you). |
| `icon` | **`icon`**, `size` (18), `color`, `fill` (false) | Name from `icons.txt`. |
| `image` | `path` *or* `base64`, `width`, `height`, `rounding` (0) | PNG, JPG or BMP. `path` absolute or relative to the plugin folder; reloaded when the file changes; scaled down past 1024 px. |
| `progress` | **`value`** (0..1), `color` (accent), `width` (fill) | Thin bar. |
| `ring` | **`value`** (0..1), `label` (""), `size` (40), `color` (accent) | Circular progress with a short label inside. |
| `sparkline` | **`values`** (array of numbers), `max` (auto), `color`, `width`, `height` | Area chart, newest value on the right. |
| `list` | **`items`**: `[{ **id**, **title**, subtitle, icon, trailing, selected }]` | Clickable rows; click sends `event` with the item's `id`. `trailing` is short right-aligned text (a time, a size). |
| `canvas` | **`height`**, `width` (fill), `id`, `keys`, `shapes` | See §5. |

## 4. Controls

All need a stable **`id`**. See protocol.md §6 for exact event payloads.

| type | Fields (default) | Sends |
|---|---|---|
| `button` | **`id`**, `label` (""), `icon`, `primary` (false), `width` | `click`. No label → round icon button. `width: 0` stretches to fill the row. `primary` = accent fill (one per view). |
| `toggle` | **`id`**, **`value`** (bool), `label` | `change` with the new bool. |
| `slider` | **`id`**, **`value`** (number), `min` (0), `max` (1), `step`, `width` | `change` while dragging, `commit` on release. |
| `input` | **`id`**, `value` (""), `hint` (""), `width` (fill), `live` (false) | `submit` on Enter; with `live`, `change` on each edit. Needs the page or a large tile to be usable. |
| `chips` | **`id`**, **`options`** (array of strings), `selected` (0) | `change` with `{index, label}`. Segmented filter like `["Day","Week","Month"]`. |

**Value reconciliation.** Nook keeps what the user did (a flipped toggle, typed text, a dragged slider) until you send a `value` that differs from the last one you sent for that `id`. Re-sending the same value is a no-op; sending a new value overrides the user. To reset an input after `submit`, give it a new id (`"new:1"` → `"new:2"`).

## 5. Canvas and shapes

`{"type":"canvas","height":104,"width":104,"id":"face","shapes":[...]}`. Coordinates are points from the canvas's top left. Without `width` the canvas fills the available width. With an `id`, clicks send `{"x","y"}`. With `"keys": true` as well, the canvas holds the keyboard while shown (unless a text field has it) and sends `key` events (see protocol.md).

| Shape `type` | Fields (default) |
|---|---|
| `rect` | **`x y w h`**, `radius` (0), `fill`, `stroke`, `width` (1) |
| `circle` | **`x y r`**, `fill`, `stroke`, `width` (1) |
| `line` | **`points`** `[[x,y],...]`, `color`, `width` (1), `closed` (false) |
| `polygon` | **`points`**, `fill` (convex shapes only) |
| `arc` | **`x y r start end`** (degrees, clockwise from 12 o'clock; `-120` to `120` is a gauge), `color`, `width` (1) |
| `text` | **`x y text`**, `size` (13), `color`, `anchor` (`start`/`center`/`end`, start; vertically centered on y), `bold` |
| `icon` | **`x y icon`**, `size` (18), `color`, `fill` |

Animate by sending a new tree from a live plugin; ~30 per second is smooth. Send nothing when nothing moves.

Python helper names for shapes: `rect circle line polygon arc label glyph` (label = text shape, glyph = icon shape). Node: `nook.shape.rect(...)`, `nook.shape.text(...)`, `nook.shape.icon(...)`.

## 6. Colors, text sizes, icons

**Colors** (any `color`, `fill`, `stroke` field):
- Theme (adapt to light/dark and the user's accent, so prefer these): `text`, `dim`, `faint`, `accent`, `on_accent` (text on an accent fill).
- Fixed: `red`, `orange`, `yellow`, `green`, `teal`, `blue`, `purple`, `pink`, `white`, `black`, `transparent`.
- Hex: `#RGB`, `#RRGGBB`, `#RRGGBBAA`. Anything else falls back to the default color silently.

**Text sizes**: `small` (captions), `body` (default), `title` (body + 3, semibold), `big`, `huge` (≈1.6× big). On a small tile, `title` or `big` for a number; `small` + `color: "dim"` for a caption.

**Icons**: Phosphor names from `icons.txt`, kebab-case (`cloud-sun`; `cloud_sun` and `CloudSun` also resolve). `fill: true` for the filled style. Unknown name → `?`.

## 7. Patterns

Icon + two lines on a wide tile:
```json
{"type":"row","align":"center","gap":10,"children":[
  {"type":"icon","icon":"cloud-sun","size":28,"color":"accent"},
  {"type":"column","gap":0,"children":[
    {"type":"text","text":"Tbilisi","size":"small","color":"dim"},
    {"type":"text","text":"21°  Sunny","size":"title"}]}]}
```

Centered number with buttons on a small tile:
```json
{"type":"column","gap":0,"align":"center","justify":"center","children":[
  {"type":"text","text":"3","size":"title"},
  {"type":"row","gap":2,"children":[
    {"type":"button","id":"dec","icon":"minus"},
    {"type":"button","id":"inc","icon":"plus","primary":true}]}]}
```

Page with an input, a filter and a list:
```json
{"type":"page","actions":[{"id":"clear","icon":"trash","tooltip":"Clear done"}],
 "view":{"type":"column","gap":8,"scroll":true,"children":[
  {"type":"input","id":"new:4","hint":"Add a task and press Enter"},
  {"type":"chips","id":"filter","options":["All","Open","Done"],"selected":0},
  {"type":"list","items":[
    {"id":"task:1","title":"Buy milk","icon":"circle","trailing":"today"},
    {"id":"task:2","title":"Call mom","icon":"check-circle","selected":true}]}]}}
```

Gauge on a canvas:
```json
{"type":"canvas","width":104,"height":104,"id":"gauge","shapes":[
  {"type":"arc","x":52,"y":56,"r":40,"start":-120,"end":120,"color":"faint","width":6},
  {"type":"arc","x":52,"y":56,"r":40,"start":-120,"end":30,"color":"accent","width":6},
  {"type":"text","x":52,"y":56,"text":"62","size":24,"anchor":"center","bold":true}]}
```
