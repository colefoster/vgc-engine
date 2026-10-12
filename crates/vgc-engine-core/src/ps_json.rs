//! A JSON value with JavaScript object semantics, for the parts of the
//! Showdown stream that PS round-trips through `JSON.parse` /
//! `JSON.stringify`: player options (`>player p1 {...}`, echoed into the
//! input log), JSON teams and the `end` record's team sets.
//!
//! Objects keep insertion order, an existing key keeps its place when
//! reassigned, and array-index keys ("0", "1", ...) come first in numeric
//! order, as in a JS object. [`Json::stringify`] is `JSON.stringify`.

/// A parsed JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn obj() -> Json {
        Json::Obj(Vec::new())
    }

    /// `JSON.parse`.
    pub fn parse(s: &str) -> Result<Json, String> {
        let mut p = Parser { s: s.as_bytes(), i: 0 };
        p.ws();
        let v = p.value()?;
        p.ws();
        if p.i != p.s.len() {
            return Err(format!("Unexpected non-whitespace character after JSON at position {}", p.i));
        }
        Ok(v)
    }

    pub fn get(&self, k: &str) -> Option<&Json> {
        match self {
            Json::Obj(v) => v.iter().find(|(key, _)| key == k).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, k: &str) -> Option<&mut Json> {
        match self {
            Json::Obj(v) => v.iter_mut().find(|(key, _)| key == k).map(|(_, v)| v),
            _ => None,
        }
    }

    /// `obj[k] = v`: in place when `k` exists, appended otherwise.
    pub fn set(&mut self, k: &str, val: Json) {
        if let Json::Obj(v) = self {
            if let Some(slot) = v.iter_mut().find(|(key, _)| key == k) {
                slot.1 = val;
            } else {
                v.push((k.to_string(), val));
            }
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// JS truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Json::Null => false,
            Json::Bool(b) => *b,
            Json::Num(n) => *n != 0.0 && !n.is_nan(),
            Json::Str(s) => !s.is_empty(),
            Json::Arr(_) | Json::Obj(_) => true,
        }
    }

    /// A string property, `""` when missing or not a string.
    pub fn str_of(&self, k: &str) -> &str {
        self.get(k).and_then(Json::as_str).unwrap_or("")
    }

    /// `JSON.stringify(value)`.
    pub fn stringify(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => out.push_str(&js_number(*n)),
            Json::Str(s) => write_str(s, out),
            Json::Arr(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            Json::Obj(o) => {
                out.push('{');
                let mut first = true;
                for (k, v) in js_key_order(o) {
                    if !first {
                        out.push(',');
                    }
                    first = false;
                    write_str(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

/// A JS array index key ("0" .. "4294967294", canonical form).
fn array_index(k: &str) -> Option<u32> {
    if k.is_empty() || (k.len() > 1 && k.starts_with('0')) || !k.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    k.parse::<u32>().ok().filter(|&n| n != u32::MAX)
}

/// JS own-property order: array indices ascending, then insertion order.
fn js_key_order(o: &[(String, Json)]) -> Vec<&(String, Json)> {
    let mut idx: Vec<&(String, Json)> = o.iter().filter(|(k, _)| array_index(k).is_some()).collect();
    idx.sort_by_key(|(k, _)| array_index(k));
    idx.extend(o.iter().filter(|(k, _)| array_index(k).is_none()));
    idx
}

/// `String(n)` for a finite number; `null` otherwise (as `JSON.stringify`).
pub fn js_number(n: f64) -> String {
    if !n.is_finite() {
        return "null".into();
    }
    if n == n.trunc() && n.abs() < 1e21 {
        if n == 0.0 {
            return "0".into();
        }
        return format!("{}", n as i128);
    }
    let a = n.abs();
    if a >= 1e21 || a < 1e-6 {
        // JS switches to exponent form here, with an explicit `+`.
        let s = format!("{n:e}");
        return match s.split_once('e') {
            Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e}"),
            _ => s,
        };
    }
    format!("{n}")
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn err<T>(&self) -> Result<T, String> {
        Err(format!("Unexpected token in JSON at position {}", self.i))
    }

    fn lit(&mut self, word: &str, v: Json) -> Result<Json, String> {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            self.err()
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        match self.s.get(self.i) {
            None => Err("Unexpected end of JSON input".into()),
            Some(b'{') => {
                self.i += 1;
                let mut o = Json::obj();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(o);
                }
                loop {
                    self.ws();
                    if self.s.get(self.i) != Some(&b'"') {
                        return self.err();
                    }
                    let k = self.string()?;
                    self.ws();
                    if self.s.get(self.i) != Some(&b':') {
                        return self.err();
                    }
                    self.i += 1;
                    self.ws();
                    let v = self.value()?;
                    o.set(&k, v);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(o);
                        }
                        _ => return self.err(),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Json::Arr(a));
                }
                loop {
                    self.ws();
                    a.push(self.value()?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(a));
                        }
                        _ => return self.err(),
                    }
                }
            }
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'n') => self.lit("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => {
                let start = self.i;
                if self.s[self.i] == b'-' {
                    self.i += 1;
                }
                while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || matches!(self.s[self.i], b'.' | b'e' | b'E' | b'+' | b'-')) {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?;
                t.parse::<f64>().map(Json::Num).or_else(|_| self.err())
            }
            _ => self.err(),
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.s.get(self.i..self.i + 4).ok_or("Bad Unicode escape in JSON")?;
        let v = u32::from_str_radix(std::str::from_utf8(h).map_err(|e| e.to_string())?, 16).map_err(|_| "Bad Unicode escape in JSON".to_string())?;
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, String> {
        self.i += 1; // opening quote
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.i < self.s.len() && self.s[self.i] != b'"' && self.s[self.i] != b'\\' {
                if self.s[self.i] < 0x20 {
                    return Err(format!("Bad control character in string literal in JSON at position {}", self.i));
                }
                self.i += 1;
            }
            out.push_str(std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?);
            match self.s.get(self.i) {
                None => return Err("Unterminated string in JSON".into()),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                _ => {
                    self.i += 1;
                    let c = *self.s.get(self.i).ok_or("Unterminated string in JSON")?;
                    self.i += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xD800..0xDC00).contains(&hi) && self.s[self.i..].starts_with(b"\\u") {
                                self.i += 2;
                                let lo = self.hex4()?;
                                0x10000 + ((hi - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF)
                            } else {
                                hi
                            };
                            out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                        }
                        _ => return Err("Bad escaped character in JSON".into()),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_like_js() {
        let src = r#"{"name":"Alice","team":"x|y","avatar":1,"z":{"b":1,"a":[1,2.5,"é\n"]},"1":true,"0":null}"#;
        let v = Json::parse(src).unwrap();
        assert_eq!(
            v.stringify(),
            r#"{"0":null,"1":true,"name":"Alice","team":"x|y","avatar":1,"z":{"b":1,"a":[1,2.5,"é\n"]}}"#
        );
        let mut v = v;
        v.set("name", Json::Str("Bob".into()));
        v.set("new", Json::Num(-0.0));
        assert!(v.stringify().contains(r#""name":"Bob""#) && v.stringify().ends_with(r#""new":0}"#));
        assert_eq!(Json::parse(r#""é😀""#).unwrap(), Json::Str("é😀".into()));
        assert_eq!(Json::Str("\u{1f}".into()).stringify(), r#""\u001f""#);
        assert_eq!(js_number(1e21), "1e+21");
        assert_eq!(js_number(0.1), "0.1");
    }
}
