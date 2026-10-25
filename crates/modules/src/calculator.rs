//! Calculator: type a sum the way you would say it and the answer shows as
//! you type.
//!
//! The engine handles `+ - × ÷ ^ !`, parentheses, implicit multiplication
//! (`2π`, `3(4+1)`), percentages (`200 + 15%` is 230, `20% of 150` is 30),
//! `mod`, functions (`sqrt`, `sin`, `ln`, `log`, `round`, ...), the
//! constants `pi`, `e`, `tau`, `phi` and `ans` (the last answer), and unit
//! conversion: `5 km to mi`, `72 f in c`, `2 GB in MB`, `90 min to h`.
//! Whole answers also show in hex and binary.
//!
//! Enter keeps the answer in the history (saved in
//! `%APPDATA%\Nook\calc.txt`); click an entry to reuse it.

use egui::text::{CCursor, CCursorRange};
use egui::{Align2, Color32, Id, Rect, ScrollArea, Sense, Ui, UiBuilder, pos2, vec2};
use nook_platform::clipboard;
use nook_ui::bento::tile_header;
use nook_ui::module::{Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_BODY, SIZE_LABEL, Theme, mix, with_alpha};
use nook_ui::{anim, icons, paint, widgets};

use crate::kit;

const PAGE: egui::Vec2 = vec2(640.0, 420.0);
const HISTORY_MAX: usize = 60;
const FIELD_ID: &str = "nook-calc-field";

// ------------------------------------------------------------- engine

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char),
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let s: String = src
        .chars()
        .map(|c| match c {
            '×' | '·' => '*',
            '÷' => '/',
            '−' | '–' => '-',
            ',' => ' ',
            c => c,
        })
        .collect();
    let s = s
        .replace('π', " pi ")
        .replace('√', " sqrt ")
        .replace('²', "^2")
        .replace('³', "^3");
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() || c == '_' {
            i += 1;
        } else if c.is_ascii_digit()
            || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            // Hex and binary literals.
            if c == '0' && matches!(chars.get(i + 1), Some('x' | 'X' | 'b' | 'B')) {
                let radix = if matches!(chars[i + 1], 'x' | 'X') {
                    16
                } else {
                    2
                };
                let start = i + 2;
                let mut j = start;
                while j < chars.len() && (chars[j].is_digit(radix) || chars[j] == '_') {
                    j += 1;
                }
                if j > start {
                    let digits: String = chars[start..j].iter().filter(|c| **c != '_').collect();
                    let v = i64::from_str_radix(&digits, radix).map_err(|_| "Number too big")?;
                    out.push(Tok::Num(v as f64));
                    i = j;
                    continue;
                }
            }
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            // Exponent: 1e3, 2.5E-4 (but not "2e" meaning 2·e).
            if i < chars.len()
                && matches!(chars[i], 'e' | 'E')
                && chars.get(i + 1).is_some_and(|c| {
                    c.is_ascii_digit()
                        || ((*c == '-' || *c == '+')
                            && chars.get(i + 2).is_some_and(char::is_ascii_digit))
                })
            {
                i += 2;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let text: String = chars[start..i].iter().collect();
            out.push(Tok::Num(
                text.parse().map_err(|_| format!("Bad number {text}"))?,
            ));
        } else if c.is_alphabetic() {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric()) {
                i += 1;
            }
            out.push(Tok::Ident(
                chars[start..i].iter().collect::<String>().to_lowercase(),
            ));
        } else if "+-*/^%!()".contains(c) {
            out.push(Tok::Op(c));
            i += 1;
        } else {
            return Err(format!("Unexpected {c}"));
        }
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Angle {
    Deg,
    Rad,
}

/// A value and whether it was written as a percentage.
#[derive(Clone, Copy)]
struct Val {
    v: f64,
    pct: bool,
}

impl Val {
    fn n(v: f64) -> Self {
        Self { v, pct: false }
    }
}

struct Parser<'a> {
    toks: &'a [Tok],
    at: usize,
    ans: f64,
    angle: Angle,
}

fn factorial(n: f64) -> Result<f64, String> {
    if n < 0.0 || n.fract() != 0.0 {
        return Err("Factorial needs a whole number".into());
    }
    if n > 170.0 {
        return Ok(f64::INFINITY);
    }
    Ok((1..=n as u64).fold(1.0, |acc, k| acc * k as f64))
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn eat_op(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Op(c)) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn eat_word(&mut self, w: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Ident(s)) if s == w) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Result<Val, String> {
        let mut lhs = self.term()?;
        loop {
            let sign = if self.eat_op('+') {
                1.0
            } else if self.eat_op('-') {
                -1.0
            } else {
                return Ok(lhs);
            };
            let rhs = self.term()?;
            // 200 + 15% adds 15% of 200.
            let add = if rhs.pct && !lhs.pct {
                lhs.v * rhs.v
            } else {
                rhs.v
            };
            lhs = Val::n(lhs.v + sign * add);
        }
    }

    /// Starts a factor without an operator: `2pi`, `3(4)`, `2 sqrt 9`.
    fn implicit(&self) -> bool {
        match self.peek() {
            Some(Tok::Num(_)) | Some(Tok::Op('(')) => true,
            Some(Tok::Ident(w)) => !matches!(w.as_str(), "mod" | "of"),
            _ => false,
        }
    }

    fn term(&mut self) -> Result<Val, String> {
        let mut lhs = self.unary()?;
        loop {
            if self.eat_op('*') {
                lhs = Val::n(lhs.v * self.unary()?.v);
            } else if self.eat_op('/') {
                let r = self.unary()?.v;
                if r == 0.0 {
                    return Err("Can't divide by zero".into());
                }
                lhs = Val::n(lhs.v / r);
            } else if self.eat_word("mod") {
                let r = self.unary()?.v;
                if r == 0.0 {
                    return Err("Can't divide by zero".into());
                }
                lhs = Val::n(lhs.v.rem_euclid(r));
            } else if self.eat_word("of") || self.implicit() {
                lhs = Val::n(lhs.v * self.unary()?.v);
            } else {
                return Ok(lhs);
            }
        }
    }

    fn unary(&mut self) -> Result<Val, String> {
        if self.eat_op('-') {
            let v = self.unary()?;
            return Ok(Val {
                v: -v.v,
                pct: v.pct,
            });
        }
        if self.eat_op('+') {
            return self.unary();
        }
        self.power()
    }

    fn power(&mut self) -> Result<Val, String> {
        let base = self.postfix()?;
        if self.eat_op('^') {
            let exp = self.unary()?;
            return Ok(Val::n(base.v.powf(exp.v)));
        }
        Ok(base)
    }

    fn postfix(&mut self) -> Result<Val, String> {
        let mut v = self.primary()?;
        loop {
            if self.eat_op('!') {
                v = Val::n(factorial(v.v)?);
            } else if self.eat_op('%') {
                v = Val {
                    v: v.v / 100.0,
                    pct: true,
                };
            } else {
                return Ok(v);
            }
        }
    }

    fn primary(&mut self) -> Result<Val, String> {
        match self.peek().cloned() {
            Some(Tok::Num(v)) => {
                self.at += 1;
                Ok(Val::n(v))
            }
            Some(Tok::Op('(')) => {
                self.at += 1;
                let v = self.expr()?;
                // A missing closing parenthesis at the end is forgiven.
                self.eat_op(')');
                Ok(v)
            }
            Some(Tok::Ident(w)) => {
                self.at += 1;
                if let Some(c) = constant(&w, self.ans) {
                    return Ok(Val::n(c));
                }
                let arg = self.power()?.v;
                function(&w, arg, self.angle).map(Val::n)
            }
            Some(Tok::Op(c)) => Err(format!("Unexpected {c}")),
            None => Err("Incomplete".into()),
        }
    }
}

fn constant(w: &str, ans: f64) -> Option<f64> {
    Some(match w {
        "pi" => std::f64::consts::PI,
        "e" => std::f64::consts::E,
        "tau" => std::f64::consts::TAU,
        "phi" => std::f64::consts::GOLDEN_RATIO,
        "ans" => ans,
        _ => return None,
    })
}

fn function(name: &str, x: f64, angle: Angle) -> Result<f64, String> {
    let to_rad = |v: f64| match angle {
        Angle::Deg => v.to_radians(),
        Angle::Rad => v,
    };
    let from_rad = |v: f64| match angle {
        Angle::Deg => v.to_degrees(),
        Angle::Rad => v,
    };
    let v = match name {
        "sqrt" => {
            if x < 0.0 {
                return Err("No square root of a negative".into());
            }
            x.sqrt()
        }
        "cbrt" => x.cbrt(),
        "sin" => to_rad(x).sin(),
        "cos" => to_rad(x).cos(),
        "tan" => to_rad(x).tan(),
        "asin" => from_rad(x.asin()),
        "acos" => from_rad(x.acos()),
        "atan" => from_rad(x.atan()),
        "sinh" => x.sinh(),
        "cosh" => x.cosh(),
        "tanh" => x.tanh(),
        "ln" => x.ln(),
        "log" => x.log10(),
        "log2" => x.log2(),
        "exp" => x.exp(),
        "abs" => x.abs(),
        "round" => x.round(),
        "floor" => x.floor(),
        "ceil" => x.ceil(),
        "sign" => x.signum(),
        _ => return Err(format!("Unknown word {name}")),
    };
    // sin(180°) is 0, not 1.2e-16.
    Ok(if v.abs() < 1e-14 { 0.0 } else { v })
}

pub fn eval(src: &str, ans: f64, angle: Angle) -> Result<f64, String> {
    let toks = tokenize(src)?;
    if toks.is_empty() {
        return Err(String::new());
    }
    let mut p = Parser {
        toks: &toks,
        at: 0,
        ans,
        angle,
    };
    let v = p.expr()?;
    if p.at < toks.len() {
        return Err(match &toks[p.at] {
            Tok::Op(c) => format!("Unexpected {c}"),
            Tok::Ident(w) => format!("Unknown word {w}"),
            Tok::Num(_) => "Missing operator".into(),
        });
    }
    if v.v.is_nan() {
        return Err("Not a number".into());
    }
    Ok(v.v)
}

// ------------------------------------------------------------- units

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dim {
    Length,
    Mass,
    Temp,
    Data,
    Time,
    Volume,
    Speed,
    Area,
}

/// Names, dimension, factor to the base unit (temperature: see `to_base`).
const UNITS: &[(&[&str], Dim, f64)] = &[
    (&["mm", "millimeter", "millimeters"], Dim::Length, 0.001),
    (&["cm", "centimeter", "centimeters"], Dim::Length, 0.01),
    (
        &["m", "meter", "meters", "metre", "metres"],
        Dim::Length,
        1.0,
    ),
    (&["km", "kilometer", "kilometers"], Dim::Length, 1000.0),
    (&["in", "inch", "inches", "\""], Dim::Length, 0.0254),
    (&["ft", "foot", "feet", "'"], Dim::Length, 0.3048),
    (&["yd", "yard", "yards"], Dim::Length, 0.9144),
    (&["mi", "mile", "miles"], Dim::Length, 1609.344),
    (&["nmi"], Dim::Length, 1852.0),
    (&["mg"], Dim::Mass, 1e-6),
    (&["g", "gram", "grams"], Dim::Mass, 0.001),
    (
        &["kg", "kilo", "kilos", "kilogram", "kilograms"],
        Dim::Mass,
        1.0,
    ),
    (&["t", "tonne", "tonnes"], Dim::Mass, 1000.0),
    (&["oz", "ounce", "ounces"], Dim::Mass, 0.028_349_523_125),
    (&["lb", "lbs", "pound", "pounds"], Dim::Mass, 0.453_592_37),
    (&["st", "stone"], Dim::Mass, 6.350_293_18),
    (&["c", "°c", "celsius"], Dim::Temp, 0.0),
    (&["f", "°f", "fahrenheit"], Dim::Temp, 1.0),
    (&["k", "kelvin"], Dim::Temp, 2.0),
    (&["bit", "bits"], Dim::Data, 0.125),
    (&["b", "byte", "bytes"], Dim::Data, 1.0),
    (&["kb"], Dim::Data, 1e3),
    (&["mb"], Dim::Data, 1e6),
    (&["gb"], Dim::Data, 1e9),
    (&["tb"], Dim::Data, 1e12),
    (&["kib"], Dim::Data, 1024.0),
    (&["mib"], Dim::Data, 1_048_576.0),
    (&["gib"], Dim::Data, 1_073_741_824.0),
    (&["tib"], Dim::Data, 1_099_511_627_776.0),
    (&["ms"], Dim::Time, 0.001),
    (&["s", "sec", "secs", "second", "seconds"], Dim::Time, 1.0),
    (&["min", "mins", "minute", "minutes"], Dim::Time, 60.0),
    (&["h", "hr", "hrs", "hour", "hours"], Dim::Time, 3600.0),
    (&["d", "day", "days"], Dim::Time, 86_400.0),
    (&["wk", "week", "weeks"], Dim::Time, 604_800.0),
    (&["yr", "year", "years"], Dim::Time, 31_557_600.0),
    (&["ml"], Dim::Volume, 0.001),
    (&["cl"], Dim::Volume, 0.01),
    (
        &["l", "liter", "liters", "litre", "litres"],
        Dim::Volume,
        1.0,
    ),
    (&["gal", "gallon", "gallons"], Dim::Volume, 3.785_411_784),
    (&["qt", "quart", "quarts"], Dim::Volume, 0.946_352_946),
    (&["pt", "pint", "pints"], Dim::Volume, 0.473_176_473),
    (&["cup", "cups"], Dim::Volume, 0.236_588_236_5),
    (&["floz"], Dim::Volume, 0.029_573_529_562_5),
    (&["tbsp"], Dim::Volume, 0.014_786_764_781_25),
    (&["tsp"], Dim::Volume, 0.004_928_921_593_75),
    (&["m/s", "mps"], Dim::Speed, 1.0),
    (&["km/h", "kmh", "kph"], Dim::Speed, 1.0 / 3.6),
    (&["mph"], Dim::Speed, 0.447_04),
    (&["kn", "knot", "knots"], Dim::Speed, 0.514_444),
    (&["m2", "sqm"], Dim::Area, 1.0),
    (&["km2"], Dim::Area, 1e6),
    (&["ha", "hectare", "hectares"], Dim::Area, 1e4),
    (&["acre", "acres"], Dim::Area, 4_046.856_422_4),
    (&["ft2", "sqft"], Dim::Area, 0.092_903_04),
];

fn unit(name: &str) -> Option<(Dim, f64, &'static str)> {
    let n = name.trim().to_lowercase();
    UNITS
        .iter()
        .find(|(names, ..)| names.contains(&n.as_str()))
        .map(|(names, d, f)| (*d, *f, names[0]))
}

fn to_base(v: f64, dim: Dim, f: f64) -> f64 {
    match (dim, f as u8) {
        (Dim::Temp, 0) => v,
        (Dim::Temp, 1) => (v - 32.0) * 5.0 / 9.0,
        (Dim::Temp, _) => v - 273.15,
        _ => v * f,
    }
}

fn from_base(v: f64, dim: Dim, f: f64) -> f64 {
    match (dim, f as u8) {
        (Dim::Temp, 0) => v,
        (Dim::Temp, 1) => v * 9.0 / 5.0 + 32.0,
        (Dim::Temp, _) => v + 273.15,
        _ => v / f,
    }
}

/// Splits `5 km to mi` into (`5`, `km`, `mi`).
fn split_conversion(src: &str) -> Option<(&str, &str, &str)> {
    let lower = src.to_lowercase();
    let at = [" to ", " as ", "->", "→"]
        .iter()
        .filter_map(|k| lower.find(k).map(|i| (i, k.len())))
        .min_by_key(|(i, _)| *i)
        .or_else(|| lower.rfind(" in ").map(|i| (i, 4)))?;
    let (lhs, rhs) = (&src[..at.0], &src[at.0 + at.1..]);
    let lhs = lhs.trim_end();
    // The unit is the trailing run of letters (and / ° " '), plus the 2
    // of an area unit (`km2`).
    let chars: Vec<(usize, char)> = lhs.char_indices().collect();
    let mut k = chars.len();
    if k > 1 && chars[k - 1].1 == '2' && chars[k - 2].1.is_alphabetic() {
        k -= 1;
    }
    while k > 0
        && (chars[k - 1].1.is_alphabetic() || matches!(chars[k - 1].1, '/' | '°' | '"' | '\''))
    {
        k -= 1;
    }
    let cut = chars.get(k)?.0;
    let (num, from) = (&lhs[..cut], &lhs[cut..]);
    Some((num.trim(), from.trim(), rhs.trim()))
}

/// The value and unit name when `src` is a conversion.
pub fn convert(src: &str, ans: f64, angle: Angle) -> Option<Result<(f64, &'static str), String>> {
    let (num, from, to) = split_conversion(src)?;
    let (fd, ff, _) = unit(from)?;
    let (td, tf, tname) = unit(to)?;
    if fd != td {
        return Some(Err(format!("Can't convert {from} to {to}")));
    }
    let v = if num.is_empty() {
        Ok(1.0)
    } else {
        eval(num, ans, angle)
    };
    Some(v.map(|v| (from_base(to_base(v, fd, ff), td, tf), tname)))
}

/// What the display shows for `src`: the value, and a unit if converted.
fn compute(src: &str, ans: f64, angle: Angle) -> Result<(f64, Option<&'static str>), String> {
    if let Some(r) = convert(src, ans, angle) {
        return r.map(|(v, u)| (v, Some(u)));
    }
    eval(src, ans, angle).map(|v| (v, None))
}

/// Groups thousands: `1234567.5` is `1,234,567.5`.
pub fn format_num(v: f64) -> String {
    if v.is_infinite() {
        return if v > 0.0 { "∞".into() } else { "-∞".into() };
    }
    let a = v.abs();
    if a != 0.0 && !(1e-7..1e15).contains(&a) {
        let s = format!("{v:.8e}");
        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
        let m = m.trim_end_matches('0').trim_end_matches('.');
        return format!("{m}e{e}");
    }
    // Twelve significant digits, trailing zeros dropped.
    let decimals = if a == 0.0 {
        0
    } else {
        (11 - a.log10().floor() as i32).clamp(0, 12) as usize
    };
    let s = format!("{:.*}", decimals, v);
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    };
    let (sign, s) = s.strip_prefix('-').map_or(("", s.as_str()), |r| ("-", r));
    let (int, frac) = s.split_once('.').map_or((s, None), |(i, f)| (i, Some(f)));
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let out = match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    };
    if out == "-0" { "0".into() } else { out }
}

/// `0xFF  ·  0b1111 1111` for whole numbers that fit.
fn radix_line(v: f64) -> Option<String> {
    if v.fract() != 0.0 || v.abs() >= 9.0e15 {
        return None;
    }
    let n = v as i64;
    if n < 0 {
        return None;
    }
    let bin = format!("{n:b}");
    let bin = if bin.len() > 32 {
        None
    } else {
        let pad = bin.len().div_ceil(4) * 4;
        let bin = format!("{n:0pad$b}");
        Some(
            bin.as_bytes()
                .chunks(4)
                .map(|c| std::str::from_utf8(c).unwrap_or(""))
                .collect::<Vec<_>>()
                .join(" "),
        )
    };
    Some(match bin {
        Some(b) => format!("0x{n:X}  \u{00B7}  0b {b}"),
        None => format!("0x{n:X}"),
    })
}

// ------------------------------------------------------------- module

#[derive(Clone, Copy)]
enum Key {
    Insert(&'static str, &'static str),
    Back,
    Clear,
    Equals,
}

#[derive(Clone, Copy, PartialEq)]
enum KeyKind {
    Digit,
    Op,
    Func,
    Action,
    Equals,
}

const KEYS: [[(Key, KeyKind); 6]; 5] = {
    use Key::*;
    use KeyKind as K;
    [
        [
            (Insert("sin", "sin("), K::Func),
            (Insert("cos", "cos("), K::Func),
            (Insert("tan", "tan("), K::Func),
            (Insert("(", "("), K::Func),
            (Insert(")", ")"), K::Func),
            (Clear, K::Action),
        ],
        [
            (Insert("ln", "ln("), K::Func),
            (Insert("7", "7"), K::Digit),
            (Insert("8", "8"), K::Digit),
            (Insert("9", "9"), K::Digit),
            (Insert("÷", "÷"), K::Op),
            (Back, K::Action),
        ],
        [
            (Insert("log", "log("), K::Func),
            (Insert("4", "4"), K::Digit),
            (Insert("5", "5"), K::Digit),
            (Insert("6", "6"), K::Digit),
            (Insert("×", "×"), K::Op),
            (Insert("%", "%"), K::Op),
        ],
        [
            (Insert("√", "√("), K::Func),
            (Insert("1", "1"), K::Digit),
            (Insert("2", "2"), K::Digit),
            (Insert("3", "3"), K::Digit),
            (Insert("−", "−"), K::Op),
            (Insert("xʸ", "^"), K::Op),
        ],
        [
            (Insert("π", "π"), K::Func),
            (Insert("0", "0"), K::Digit),
            (Insert(".", "."), K::Digit),
            (Insert("ans", "ans"), K::Func),
            (Insert("+", "+"), K::Op),
            (Equals, K::Equals),
        ],
    ]
};

struct Entry {
    expr: String,
    result: String,
}

pub struct Calculator {
    expr: String,
    angle: Angle,
    ans: f64,
    history: Vec<Entry>,
    open_page: bool,
    show: bool,
    focus: bool,
    /// Flashes the result after Enter.
    committed_at: f64,
}

impl Default for Calculator {
    fn default() -> Self {
        Self {
            expr: String::new(),
            angle: Angle::Deg,
            ans: 0.0,
            history: Vec::new(),
            open_page: false,
            show: false,
            focus: false,
            committed_at: f64::NEG_INFINITY,
        }
    }
}

fn history_file() -> Option<std::path::PathBuf> {
    kit::settings_path("calc.txt")
}

impl Calculator {
    fn load(&mut self) {
        let Some(text) = history_file().and_then(|p| std::fs::read_to_string(p).ok()) else {
            return;
        };
        for line in text.lines() {
            if let Some(a) = line.strip_prefix("#angle=") {
                self.angle = if a == "rad" { Angle::Rad } else { Angle::Deg };
            } else if let Some((e, r)) = line.split_once('\t') {
                self.history.push(Entry {
                    expr: e.to_owned(),
                    result: r.to_owned(),
                });
            }
        }
        if let Some(e) = self.history.first() {
            self.ans = parse_display(&e.result).unwrap_or(0.0);
        }
    }

    fn save(&self) {
        let Some(path) = history_file() else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut text = format!(
            "#angle={}\n",
            if self.angle == Angle::Rad {
                "rad"
            } else {
                "deg"
            }
        );
        for e in &self.history {
            text.push_str(&format!(
                "{}\t{}\n",
                e.expr.replace(['\t', '\n'], " "),
                e.result
            ));
        }
        let _ = std::fs::write(path, text);
    }

    fn result(&self) -> Result<(f64, Option<&'static str>), String> {
        compute(&self.expr, self.ans, self.angle)
    }

    fn commit(&mut self, now: f64) {
        let Ok((v, unit)) = self.result() else {
            return;
        };
        let shown = match unit {
            Some(u) => format!("{} {u}", format_num(v)),
            None => format_num(v),
        };
        self.history.insert(
            0,
            Entry {
                expr: self.expr.trim().to_owned(),
                result: shown,
            },
        );
        self.history.truncate(HISTORY_MAX);
        self.ans = v;
        // Keep going from the answer.
        self.expr = raw_num(v);
        self.committed_at = now;
        self.save();
    }

    /// Inserts `s` at the field's cursor and keeps the cursor after it.
    fn insert(&mut self, ctx: &egui::Context, s: &str) {
        let id = Id::new(FIELD_ID);
        let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        let len = self.expr.chars().count();
        let at = state
            .cursor
            .char_range()
            .map_or(len, |r| r.primary.index.into())
            .min(len);
        let byte = self
            .expr
            .char_indices()
            .nth(at)
            .map_or(self.expr.len(), |(b, _)| b);
        self.expr.insert_str(byte, s);
        let to = at + s.chars().count();
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(to))));
        state.store(ctx, id);
        self.focus = true;
    }

    fn backspace(&mut self, ctx: &egui::Context) {
        let id = Id::new(FIELD_ID);
        let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        let len = self.expr.chars().count();
        let at = state
            .cursor
            .char_range()
            .map_or(len, |r| r.primary.index.into())
            .min(len);
        if at > 0 {
            let mut chars: Vec<char> = self.expr.chars().collect();
            chars.remove(at - 1);
            self.expr = chars.into_iter().collect();
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(at - 1))));
            state.store(ctx, id);
        }
        self.focus = true;
    }

    fn cursor_to_end(&self, ctx: &egui::Context) {
        let id = Id::new(FIELD_ID);
        let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(
                self.expr.chars().count(),
            ))));
        state.store(ctx, id);
    }

    fn display(&mut self, ui: &mut Ui, card: Rect, theme: &Theme) {
        widgets::card(ui.painter(), card, theme);
        let ctx = ui.ctx().clone();
        let inner = card.shrink2(vec2(16.0, 12.0));
        let id = Id::new(FIELD_ID);
        let field = Rect::from_min_size(inner.min, vec2(inner.width(), 26.0));
        let mut fui = ui.new_child(UiBuilder::new().max_rect(field).id_salt("calc-field"));
        let resp = fui.add(
            egui::TextEdit::singleline(&mut self.expr)
                .id(id)
                .frame(egui::Frame::NONE)
                .font(Theme::font(SIZE_BODY + 4.0))
                .text_color(theme.text_dim)
                .hint_text(
                    egui::RichText::new("Type a sum, e.g. 12% of 80 or 5 km to mi")
                        .color(theme.text_faint),
                )
                .desired_width(field.width()),
        );
        if std::mem::take(&mut self.focus) {
            resp.request_focus();
        }
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.commit(ui.input(|i| i.time));
            self.cursor_to_end(&ctx);
            resp.request_focus();
        }

        let now = ui.input(|i| i.time);
        let flash = (1.0 - ((now - self.committed_at) as f32 / 0.5)).clamp(0.0, 1.0);
        if flash > 0.0 {
            ctx.request_repaint();
        }
        let result_y = inner.max.y - 30.0;
        match self.result() {
            Ok((v, unit)) => {
                let text = match unit {
                    Some(u) => format!("{} {u}", format_num(v)),
                    None => format_num(v),
                };
                let color = mix(theme.text, theme.accent_fg(), flash);
                let galley = ui.painter().layout_no_wrap(
                    format!("= {text}"),
                    Theme::font_semibold(30.0 + 2.0 * flash),
                    color,
                );
                // Shrink long answers to fit.
                let k = (inner.width() / galley.size().x).min(1.0);
                let font = Theme::font_semibold((30.0 + 2.0 * flash) * k);
                ui.painter().text(
                    pos2(inner.max.x, result_y),
                    Align2::RIGHT_CENTER,
                    format!("= {text}"),
                    font,
                    color,
                );
                let sub = if unit.is_none() { radix_line(v) } else { None };
                if let Some(sub) = sub {
                    ui.painter().text(
                        pos2(inner.max.x, inner.max.y - 4.0),
                        Align2::RIGHT_CENTER,
                        sub,
                        Theme::font(SIZE_LABEL - 1.0),
                        theme.text_faint,
                    );
                }
                let copy =
                    Rect::from_center_size(pos2(inner.min.x + 10.0, result_y), vec2(24.0, 24.0));
                if kit::icon_at(ui, copy.center(), 24.0, icons::COPY, false, theme)
                    .on_hover_text("Copy the answer")
                    .clicked()
                {
                    let _ = clipboard::copy_text(&raw_num(v));
                }
            }
            Err(e) if !self.expr.trim().is_empty() && !e.is_empty() => {
                ui.painter().text(
                    pos2(inner.max.x, result_y),
                    Align2::RIGHT_CENTER,
                    e,
                    Theme::font(SIZE_BODY),
                    theme.text_faint,
                );
            }
            Err(_) => {
                ui.painter().text(
                    pos2(inner.max.x, result_y),
                    Align2::RIGHT_CENTER,
                    "0",
                    Theme::font_semibold(30.0),
                    theme.text_faint,
                );
            }
        }
    }

    fn keypad(&mut self, ui: &mut Ui, area: Rect, theme: &Theme) {
        let ctx = ui.ctx().clone();
        let gap = 6.0;
        let rows = KEYS.len() as f32;
        let cols = KEYS[0].len() as f32;
        let kw = (area.width() - gap * (cols - 1.0)) / cols;
        let kh = (area.height() - gap * (rows - 1.0)) / rows;
        for (r, row) in KEYS.iter().enumerate() {
            for (c, (key, kind)) in row.iter().enumerate() {
                let rect = Rect::from_min_size(
                    pos2(
                        area.min.x + c as f32 * (kw + gap),
                        area.min.y + r as f32 * (kh + gap),
                    ),
                    vec2(kw, kh),
                );
                if key_button(ui, rect, *key, *kind, (r, c), theme).clicked() {
                    match key {
                        Key::Insert(_, s) => self.insert(&ctx, s),
                        Key::Back => self.backspace(&ctx),
                        Key::Clear => {
                            self.expr.clear();
                            self.focus = true;
                        }
                        Key::Equals => {
                            self.commit(ui.input(|i| i.time));
                            self.cursor_to_end(&ctx);
                            self.focus = true;
                        }
                    }
                }
            }
        }
    }

    fn history_list(&mut self, ui: &mut Ui, area: Rect, theme: &Theme) {
        kit::section(ui, area.min + vec2(4.0, 0.0), "HISTORY", theme);
        let list = Rect::from_min_max(area.min + vec2(0.0, 20.0), area.max);
        if self.history.is_empty() {
            ui.painter().text(
                list.center() - vec2(0.0, 20.0),
                Align2::CENTER_CENTER,
                "Press Enter to keep an answer",
                Theme::font(SIZE_LABEL),
                theme.text_faint,
            );
            return;
        }
        let row_h = 44.0;
        let mut pick = None;
        let mut lui = ui.new_child(UiBuilder::new().max_rect(list).id_salt("calc-history"));
        ScrollArea::vertical()
            .id_salt("nook-calc-history")
            .max_height(list.height())
            .auto_shrink([false, false])
            .show(&mut lui, |ui| {
                let (rows, _) = ui.allocate_exact_size(
                    vec2(list.width(), self.history.len() as f32 * row_h),
                    Sense::hover(),
                );
                for (i, e) in self.history.iter().enumerate() {
                    let r = Rect::from_min_size(
                        pos2(rows.min.x, rows.min.y + i as f32 * row_h),
                        vec2(rows.width(), row_h - 4.0),
                    );
                    if !ui.is_rect_visible(r) {
                        continue;
                    }
                    let resp = kit::row(ui, r, Id::new(("calc-row", i)), false, theme)
                        .on_hover_text("Click to use  \u{00B7}  Right click to copy");
                    kit::one_line(
                        ui,
                        pos2(r.min.x + 10.0, r.min.y + 4.0),
                        &e.expr,
                        r.width() - 20.0,
                        Theme::font(SIZE_LABEL - 1.0),
                        theme.text_faint,
                    );
                    kit::one_line(
                        ui,
                        pos2(r.min.x + 10.0, r.min.y + 18.0),
                        &e.result,
                        r.width() - 20.0,
                        Theme::font_semibold(SIZE_BODY),
                        theme.text,
                    );
                    if resp.clicked() {
                        pick = Some((i, false));
                    } else if resp.secondary_clicked() {
                        pick = Some((i, true));
                    }
                }
            });
        if let Some((i, copy)) = pick {
            let result = &self.history[i].result;
            let raw = result.replace(',', "");
            if copy {
                let _ = clipboard::copy_text(&raw);
            } else {
                // A converted answer is reused with its unit.
                self.insert(ui.ctx(), &raw);
            }
        }
    }
}

/// A number as the engine reads it back (no grouping).
fn raw_num(v: f64) -> String {
    format_num(v).replace(',', "")
}

fn parse_display(s: &str) -> Option<f64> {
    s.split_whitespace().next()?.replace(',', "").parse().ok()
}

fn key_button(
    ui: &mut Ui,
    rect: Rect,
    key: Key,
    kind: KeyKind,
    at: (usize, usize),
    theme: &Theme,
) -> egui::Response {
    let id = Id::new(("calc-key", at));
    let resp = ui.interact(rect, id, Sense::click());
    let ctx = ui.ctx();
    let hover = anim::tween(ctx, id.with("h"), resp.hovered(), 0.10);
    let press = anim::spring(
        ctx,
        id.with("p"),
        if resp.is_pointer_button_down_on() {
            1.0
        } else {
            0.0
        },
        nook_ui::spring::SpringParams::critical(900.0),
    );
    let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.05 * press));
    let painter = ui.painter();
    let base = |a: u8| {
        if theme.dark {
            Color32::from_white_alpha(a)
        } else {
            Color32::from_black_alpha(a / 2 + 2)
        }
    };
    let (fill, fg) = match kind {
        KeyKind::Equals => (
            mix(theme.accent, Color32::WHITE, 0.15 * hover),
            Color32::WHITE,
        ),
        KeyKind::Op => (
            with_alpha(
                theme.accent,
                if theme.dark { 0.22 } else { 0.14 } + 0.12 * hover,
            ),
            theme.accent_fg(),
        ),
        KeyKind::Digit => (mix(base(22), base(40), hover), theme.text),
        KeyKind::Func => (mix(base(10), base(28), hover), theme.text_dim),
        KeyKind::Action => (mix(base(10), base(28), hover), theme.text),
    };
    painter.rect_filled(r, 11.0, fill);
    let font = match kind {
        KeyKind::Digit | KeyKind::Equals | KeyKind::Op => Theme::font_semibold(SIZE_BODY + 3.0),
        _ => Theme::font_semibold(SIZE_LABEL + 1.0),
    };
    match key {
        Key::Insert(label, _) => {
            painter.text(r.center(), Align2::CENTER_CENTER, label, font, fg);
        }
        Key::Clear => {
            painter.text(r.center(), Align2::CENTER_CENTER, "AC", font, fg);
        }
        Key::Back => paint::icon(painter, r.center(), icons::BACKSPACE, 17.0, fg, false),
        Key::Equals => paint::icon(painter, r.center(), icons::EQUALS, 18.0, fg, true),
    }
    resp
}

impl Module for Calculator {
    fn id(&self) -> &'static str {
        "calculator"
    }

    fn name(&self) -> &str {
        "Calculator"
    }

    fn icon(&self) -> &'static str {
        icons::CALCULATOR
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn attach(&mut self, _ctx: &egui::Context) {
        self.load();
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.open_page)
    }

    fn take_show_request(&mut self) -> bool {
        std::mem::take(&mut self.show)
    }

    fn page_size(&self) -> Option<egui::Vec2> {
        Some(PAGE)
    }

    fn default_hotkey(&self) -> Option<&'static str> {
        Some("ctrl+alt+KeyK")
    }

    fn on_hotkey(&mut self) {
        self.show = true;
        self.focus = true;
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let theme = ctx.theme;
        let rect = ui.max_rect();
        if rect.width() > rect.height() * 1.5 {
            // Wide: the last answer.
            tile_header(ui, icons::CALCULATOR, "Calculator", false, theme);
            let resp = ui.interact(rect.expand(12.0), ui.id().with("calc-tile"), Sense::click());
            let last = self
                .history
                .first()
                .map_or("Type a sum".to_owned(), |e| e.result.clone());
            kit::one_line(
                ui,
                pos2(rect.min.x, rect.min.y + 21.0),
                &last,
                rect.width(),
                Theme::font_semibold(SIZE_BODY),
                if ctx.hovered {
                    theme.text
                } else {
                    theme.text_dim
                },
            );
            if resp.clicked() {
                self.open_page = true;
                self.focus = true;
            }
            return;
        }
        if kit::tool_tile(ui, ctx, icons::CALCULATOR, false)
            .on_hover_text("Calculator and unit converter  \u{00B7}  Ctrl+Alt+K")
            .clicked()
        {
            self.open_page = true;
            self.focus = true;
        }
    }

    fn ui_page(&mut self, ui: &mut Ui, header: &mut Ui, body: Rect, theme: &Theme) -> bool {
        if !self.history.is_empty()
            && widgets::icon_button(header, icons::BROOM, 26.0, false, theme)
                .on_hover_text("Clear the history")
                .clicked()
        {
            self.history.clear();
            self.save();
        }
        let mut sel = usize::from(self.angle == Angle::Rad);
        if widgets::segmented(header, &["DEG", "RAD"], &mut sel, theme)
            .on_hover_text("Angles for sin, cos and tan")
            .changed()
        {
            self.angle = if sel == 1 { Angle::Rad } else { Angle::Deg };
            self.save();
        }

        let left_w = 380.0;
        let display = Rect::from_min_size(body.min, vec2(left_w, 108.0));
        self.display(ui, display, theme);
        let pad = Rect::from_min_max(
            pos2(body.min.x, display.max.y + 10.0),
            pos2(body.min.x + left_w, body.max.y),
        );
        self.keypad(ui, pad, theme);
        let hist = Rect::from_min_max(pos2(body.min.x + left_w + 14.0, body.min.y), body.max);
        self.history_list(ui, hist, theme);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> f64 {
        eval(s, 7.0, Angle::Deg).unwrap()
    }

    #[test]
    fn arithmetic() {
        assert_eq!(ev("1 + 2 * 3"), 7.0);
        assert_eq!(ev("(1 + 2) * 3"), 9.0);
        assert_eq!(ev("2^3^2"), 512.0);
        assert_eq!(ev("-2^2"), -4.0);
        assert_eq!(ev("10 ÷ 4"), 2.5);
        assert_eq!(ev("3 × 4 − 2"), 10.0);
        assert_eq!(ev("5!"), 120.0);
        assert_eq!(ev("17 mod 5"), 2.0);
        assert_eq!(ev("0xFF + 0b11"), 258.0);
        assert_eq!(ev("1e3 + 2.5E-1"), 1000.25);
        assert_eq!(ev("ans * 2"), 14.0);
        assert_eq!(ev("(2 + 3"), 5.0);
    }

    #[test]
    fn implicit_and_functions() {
        assert!((ev("2pi") - std::f64::consts::TAU).abs() < 1e-12);
        assert_eq!(ev("3(4+1)"), 15.0);
        assert_eq!(ev("sqrt 16"), 4.0);
        assert_eq!(ev("√(9)"), 3.0);
        assert!((ev("sin(30)") - 0.5).abs() < 1e-12);
        assert_eq!(ev("sin(180)"), 0.0);
        assert_eq!(ev("log(1000)"), 3.0);
        assert!((eval("cos(pi)", 0.0, Angle::Rad).unwrap() + 1.0).abs() < 1e-12);
        assert!(eval("2 +", 0.0, Angle::Deg).is_err());
        assert!(eval("1/0", 0.0, Angle::Deg).is_err());
        assert!(eval("foo(2)", 0.0, Angle::Deg).is_err());
    }

    #[test]
    fn percentages() {
        assert_eq!(ev("200 + 15%"), 230.0);
        assert_eq!(ev("200 - 10%"), 180.0);
        assert_eq!(ev("20% of 150"), 30.0);
        assert_eq!(ev("50%"), 0.5);
    }

    #[test]
    fn conversions() {
        let c = |s: &str| convert(s, 0.0, Angle::Deg).unwrap().unwrap();
        assert!((c("5 km to mi").0 - 3.106_855_96).abs() < 1e-6);
        assert_eq!(c("100 c to f"), (212.0, "f"));
        assert!((c("72 f in c").0 - 22.222_222).abs() < 1e-5);
        assert_eq!(c("2 GB in MB"), (2000.0, "mb"));
        assert_eq!(c("90 min to h"), (1.5, "h"));
        assert!((c("10 in in cm").0 - 25.4).abs() < 1e-9);
        assert!((c("2*3 kg to lb").0 - 13.227_735_731).abs() < 1e-6);
        assert!(convert("5 km to kg", 0.0, Angle::Deg).unwrap().is_err());
        assert!(convert("1 + 2", 0.0, Angle::Deg).is_none());
    }

    #[test]
    fn formatting() {
        assert_eq!(format_num(1_234_567.5), "1,234,567.5");
        assert_eq!(format_num(-1234.0), "-1,234");
        assert_eq!(format_num(0.1 + 0.2), "0.3");
        assert_eq!(format_num(1.0 / 3.0), "0.333333333333");
        assert_eq!(format_num(2e20), "2e20");
        assert_eq!(format_num(0.0), "0");
        assert_eq!(radix_line(255.0).unwrap(), "0xFF  \u{00B7}  0b 1111 1111");
        assert!(radix_line(2.5).is_none());
        assert_eq!(parse_display("1,234.5 mb"), Some(1234.5));
    }
}
