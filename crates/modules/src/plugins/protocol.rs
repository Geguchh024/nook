//! The Nook plugin protocol, version 1.
//!
//! A plugin prints one JSON object per line on stdout ([`Out`]) and, in live
//! mode, reads one per line on stdin (the `hello`, `event`, ... messages
//! built by the `msg_*` functions below). Views are trees of [`Node`]s that
//! Nook draws in its own style; plugins never draw pixels themselves except
//! on a `canvas`.

use serde::Deserialize;
use serde_json::{Value, json};

pub const PROTOCOL: u32 = 1;

/// A message from the plugin.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Out {
    /// Replaces the tile's content.
    Tile {
        view: Node,
    },
    /// Replaces the page's content (and makes a tile click open the page).
    Page {
        view: Node,
        /// Round buttons in the page header, right to left.
        #[serde(default)]
        actions: Vec<Action>,
    },
    /// What the collapsed pill shows for this plugin; `null` clears it.
    Badge {
        badge: Option<BadgeSpec>,
    },
    /// Lights the tile with the accent.
    Active {
        active: bool,
    },
    /// Opens the page if the panel is open.
    OpenPage,
    ClosePage,
    /// Opens the panel, pinned, on this plugin's page.
    Show,
    /// Puts text on the clipboard.
    Copy {
        text: String,
    },
    /// Opens a URL, file or folder with its default app.
    Open {
        target: String,
    },
    Log {
        #[serde(default)]
        level: String,
        message: String,
    },
}

/// The `type` values of [`Out`]; anything else on a line is a view.
const OUT_TYPES: &[&str] = &[
    "tile",
    "page",
    "badge",
    "active",
    "open_page",
    "close_page",
    "show",
    "copy",
    "open",
    "log",
];

#[derive(Clone, Debug, Deserialize)]
pub struct Action {
    pub id: String,
    pub icon: String,
    #[serde(default)]
    pub tooltip: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BadgeSpec {
    /// Progress ring (0..1) with a short label, replacing the pill clock.
    Ring {
        progress: f32,
        #[serde(default)]
        label: String,
    },
    /// A small icon on the right of the pill.
    Glyph { icon: String },
    /// A pulsing dot: something arrived.
    Pulse,
}

/// One node of a view tree. `tooltip` and `drag` work on every node.
#[derive(Clone, Debug, Deserialize)]
pub struct Node {
    #[serde(flatten)]
    pub kind: Kind,
    #[serde(default)]
    pub tooltip: Option<String>,
    /// Files to drag out of Nook when the node is dragged (as from
    /// Explorer); the plugin then gets a `dragged` message.
    #[serde(default)]
    pub drag: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Kind {
    // Layout
    Column {
        #[serde(default)]
        children: Vec<Node>,
        gap: Option<f32>,
        /// Cross-axis alignment: start, center, end.
        #[serde(default)]
        align: Align,
        /// Main-axis placement inside the available height.
        #[serde(default)]
        justify: Align,
        /// Scroll when taller than the room (pages). With justify "end" the
        /// scroll sticks to the bottom.
        #[serde(default)]
        scroll: bool,
        /// Keys the scroll position: a new id starts a fresh scroll (at the
        /// bottom with justify "end"). Columns without one share it.
        id: Option<String>,
    },
    Row {
        #[serde(default)]
        children: Vec<Node>,
        gap: Option<f32>,
        #[serde(default)]
        align: Align,
    },
    Grid {
        columns: usize,
        #[serde(default)]
        children: Vec<Node>,
        gap: Option<f32>,
    },
    Card {
        #[serde(default)]
        children: Vec<Node>,
        padding: Option<f32>,
        gap: Option<f32>,
    },
    /// A fixed gap, or (no size) pushes what follows to the far end of the
    /// row or column.
    Spacer {
        size: Option<f32>,
    },
    Divider,

    // Content
    Text {
        text: String,
        #[serde(default)]
        size: TextSize,
        color: Option<String>,
        #[serde(default)]
        bold: bool,
        /// Wrap onto more lines instead of ending with "…".
        #[serde(default)]
        wrap: bool,
        /// Monospaced digits and letters.
        #[serde(default)]
        mono: bool,
    },
    Icon {
        icon: String,
        size: Option<f32>,
        color: Option<String>,
        #[serde(default)]
        fill: bool,
    },
    Image {
        /// A PNG, JPG or BMP file, absolute or relative to the plugin folder.
        path: Option<String>,
        /// Or the file's bytes in base64.
        base64: Option<String>,
        width: Option<f32>,
        height: Option<f32>,
        #[serde(default)]
        rounding: f32,
    },
    Progress {
        value: f32,
        color: Option<String>,
        width: Option<f32>,
    },
    Ring {
        value: f32,
        #[serde(default)]
        label: String,
        size: Option<f32>,
        color: Option<String>,
    },
    Sparkline {
        values: Vec<f32>,
        max: Option<f32>,
        color: Option<String>,
        width: Option<f32>,
        height: Option<f32>,
    },
    List {
        items: Vec<ListItem>,
    },
    Canvas {
        /// Click events carry this id plus `x` and `y` in canvas points.
        id: Option<String>,
        width: Option<f32>,
        height: f32,
        #[serde(default)]
        shapes: Vec<Shape>,
        /// With an id: the canvas holds the keyboard while it is shown (unless
        /// a text field has it) and sends `key` events (games).
        #[serde(default)]
        keys: bool,
    },

    // Controls: each sends `event` messages with its `id`.
    Button {
        id: String,
        #[serde(default)]
        label: String,
        icon: Option<String>,
        #[serde(default)]
        primary: bool,
        width: Option<f32>,
    },
    Toggle {
        id: String,
        value: bool,
        label: Option<String>,
    },
    Slider {
        id: String,
        value: f64,
        #[serde(default)]
        min: f64,
        #[serde(default = "one")]
        max: f64,
        step: Option<f64>,
        width: Option<f32>,
    },
    Input {
        id: String,
        #[serde(default)]
        value: String,
        #[serde(default)]
        hint: String,
        width: Option<f32>,
        /// Send `change` on every edit, not only `submit` on Enter.
        #[serde(default)]
        live: bool,
    },
    Chips {
        id: String,
        options: Vec<String>,
        #[serde(default)]
        selected: usize,
    },

    /// A type this version of Nook does not know: drawn as nothing.
    #[serde(other)]
    Unknown,
}

fn one() -> f64 {
    1.0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextSize {
    Small,
    #[default]
    Body,
    Title,
    Big,
    Huge,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ListItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    pub icon: Option<String>,
    /// Short text on the right (a time, a size, a count).
    #[serde(default)]
    pub trailing: String,
    #[serde(default)]
    pub selected: bool,
}

/// A canvas primitive; coordinates are points from the canvas' top left.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Shape {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        #[serde(default)]
        radius: f32,
        fill: Option<String>,
        stroke: Option<String>,
        #[serde(default = "one_f32")]
        width: f32,
    },
    Circle {
        x: f32,
        y: f32,
        r: f32,
        fill: Option<String>,
        stroke: Option<String>,
        #[serde(default = "one_f32")]
        width: f32,
    },
    Line {
        points: Vec<[f32; 2]>,
        color: Option<String>,
        #[serde(default = "one_f32")]
        width: f32,
        #[serde(default)]
        closed: bool,
    },
    Polygon {
        points: Vec<[f32; 2]>,
        fill: Option<String>,
    },
    /// Angles in degrees, clockwise from 12 o'clock.
    Arc {
        x: f32,
        y: f32,
        r: f32,
        start: f32,
        end: f32,
        color: Option<String>,
        #[serde(default = "one_f32")]
        width: f32,
    },
    Text {
        x: f32,
        y: f32,
        text: String,
        #[serde(default = "body_size")]
        size: f32,
        color: Option<String>,
        /// Which point of the text sits on (x, y): `left`, `center` or
        /// `right` (vertically centered).
        #[serde(default)]
        anchor: Align,
        #[serde(default)]
        bold: bool,
    },
    Icon {
        x: f32,
        y: f32,
        icon: String,
        #[serde(default = "icon_size")]
        size: f32,
        color: Option<String>,
        #[serde(default)]
        fill: bool,
    },
    #[serde(other)]
    Unknown,
}

fn one_f32() -> f32 {
    1.0
}
fn body_size() -> f32 {
    13.0
}
fn icon_size() -> f32 {
    18.0
}

/// Parses one line (or a whole poll output) from a plugin. A bare view node
/// is shorthand for `{"type": "tile", "view": ...}`.
pub fn parse(text: &str) -> Result<Out, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let ty = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if OUT_TYPES.contains(&ty.as_str()) {
        Out::deserialize(value).map_err(|e| format!("bad {ty:?} message: {e}"))
    } else if ty.is_empty() {
        Err("message has no \"type\"".into())
    } else {
        Node::deserialize(value)
            .map(|view| Out::Tile { view })
            .map_err(|e| format!("bad {ty:?} view: {e}"))
    }
}

/// Everything a poll run printed: the whole output as one JSON document, or
/// one message per line.
pub fn parse_output(text: &str) -> Vec<Result<Out, String>> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if let Ok(out) = parse(text) {
        return vec![Ok(out)];
    }
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(parse)
        .collect()
}

// Messages to the plugin.

pub struct Hello<'a> {
    pub id: &'a str,
    pub data_dir: &'a str,
    pub dark: bool,
    pub accent: &'a str,
}

pub fn msg_hello(h: &Hello<'_>) -> Value {
    json!({
        "type": "hello",
        "protocol": PROTOCOL,
        "nook_version": env!("CARGO_PKG_VERSION"),
        "plugin_id": h.id,
        "data_dir": h.data_dir,
        "theme": { "dark": h.dark, "accent": h.accent },
    })
}

pub fn msg_theme(dark: bool, accent: &str) -> Value {
    json!({ "type": "theme", "dark": dark, "accent": accent })
}

/// The room a view has, in points: `what` is `tile` or `page`.
pub fn msg_layout(what: &str, size: &str, width: f32, height: f32) -> Value {
    json!({ "type": "layout", "target": what, "size": size, "width": width.round(), "height": height.round() })
}

pub fn msg_event(id: &str, kind: &str, value: Value) -> Value {
    let mut m = json!({ "type": "event", "id": id, "kind": kind });
    if !value.is_null() {
        m["value"] = value;
    }
    m
}

pub fn msg_simple(ty: &str) -> Value {
    json!({ "type": ty })
}

pub fn msg_files(paths: &[std::path::PathBuf]) -> Value {
    let paths: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    json!({ "type": "files", "paths": paths })
}

/// A `drag` node's files were dragged out: `result` is `copied`, `moved`,
/// `linked`, `cancelled` or `error` (with `error` set).
pub fn msg_dragged(paths: &[String], result: &str, error: Option<&str>) -> Value {
    let mut m = json!({ "type": "dragged", "paths": paths, "result": result });
    if let Some(e) = error {
        m["error"] = json!(e);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_messages_and_bare_views() {
        let m = parse(r#"{"type":"tile","view":{"type":"text","text":"hi"}}"#).unwrap();
        assert!(matches!(m, Out::Tile { .. }));
        let m = parse(r#"{"type":"text","text":"hi","tooltip":"t"}"#).unwrap();
        let Out::Tile { view } = m else {
            panic!("not a tile")
        };
        assert_eq!(view.tooltip.as_deref(), Some("t"));
        assert!(matches!(view.kind, Kind::Text { .. }));
        assert!(view.drag.is_none());
        let m = parse(r#"{"type":"card","children":[],"drag":["C:\\a.txt"]}"#).unwrap();
        let Out::Tile { view } = m else {
            panic!("not a tile")
        };
        assert_eq!(view.drag, Some(vec![r"C:\a.txt".to_owned()]));
        let m = parse(r#"{"type":"badge","badge":{"kind":"ring","progress":0.5}}"#).unwrap();
        assert!(matches!(
            m,
            Out::Badge {
                badge: Some(BadgeSpec::Ring { .. })
            }
        ));
        assert!(matches!(
            parse(r#"{"type":"badge","badge":null}"#).unwrap(),
            Out::Badge { badge: None }
        ));
    }

    #[test]
    fn unknown_nodes_degrade_and_errors_explain() {
        let m = parse(r#"{"type":"row","children":[{"type":"hologram","x":1}]}"#).unwrap();
        let Out::Tile { view } = m else {
            panic!("not a tile")
        };
        let Kind::Row { children, .. } = view.kind else {
            panic!("not a row")
        };
        assert!(matches!(children[0].kind, Kind::Unknown));
        let e = parse(r#"{"type":"button","label":"no id"}"#).unwrap_err();
        assert!(e.contains("button"), "{e}");
        assert!(parse("nope").unwrap_err().contains("JSON"));
    }

    #[test]
    fn poll_output_whole_or_lines() {
        let pretty = "{\n \"type\": \"text\",\n \"text\": \"a\"\n}\n";
        assert_eq!(parse_output(pretty).len(), 1);
        let lines = "{\"type\":\"active\",\"active\":true}\n{\"type\":\"text\",\"text\":\"a\"}\n";
        let out = parse_output(lines);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(Result::is_ok));
    }
}
