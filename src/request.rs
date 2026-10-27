//! Small, bounded JSON request grammar: flat objects, strings, integer i64s,
//! booleans and string arrays. Reject duplicate keys, floats and trailing input.
use std::collections::BTreeMap;

#[derive(Debug, PartialEq)]
enum Value {
    Text(String),
    Integer(i64),
    Flag(bool),
    Strings(Vec<String>),
}

#[derive(Default)]
pub struct Fields(BTreeMap<String, Value>);

impl Fields {
    pub fn has(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }
    pub fn text(&self, key: &str) -> Option<&str> {
        match self.0.get(key)? {
            Value::Text(v) => Some(v),
            _ => None,
        }
    }
    pub fn integer(&self, key: &str) -> Option<i64> {
        match self.0.get(key)? {
            Value::Integer(v) => Some(*v),
            _ => None,
        }
    }
    pub fn flag(&self, key: &str) -> Option<bool> {
        match self.0.get(key)? {
            Value::Flag(v) => Some(*v),
            _ => None,
        }
    }
    pub fn strings(&self, key: &str) -> &[String] {
        match self.0.get(key) {
            Some(Value::Strings(v)) => v,
            _ => &[],
        }
    }
}

struct Parser<'a>(&'a str);
impl Parser<'_> {
    fn ws(&mut self) {
        self.0 = self.0.trim_start_matches([' ', '\t', '\r', '\n']);
    }
    fn eat(&mut self, prefix: &str) -> bool {
        self.ws();
        if let Some(rest) = self.0.strip_prefix(prefix) {
            self.0 = rest;
            true
        } else {
            false
        }
    }
    fn hex4(&mut self) -> Option<u32> {
        let digits = self.0.get(..4)?;
        if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let code = u32::from_str_radix(digits, 16).ok()?;
        self.0 = &self.0[4..];
        Some(code)
    }
    fn string(&mut self) -> Option<String> {
        if !self.eat("\"") {
            return None;
        }
        let mut out = String::new();
        loop {
            let ch = self.0.chars().next()?;
            self.0 = &self.0[ch.len_utf8()..];
            match ch {
                '"' => return Some(out),
                '\\' => {
                    let escape = self.0.chars().next()?;
                    self.0 = &self.0[escape.len_utf8()..];
                    out.push(match escape {
                        '"' | '\\' | '/' => escape,
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        'b' => '\u{8}',
                        'f' => '\u{c}',
                        'u' => {
                            let mut code = self.hex4()?;
                            if (0xd800..=0xdbff).contains(&code) {
                                self.0 = self.0.strip_prefix("\\u")?;
                                let low = self.hex4()?;
                                if !(0xdc00..=0xdfff).contains(&low) {
                                    return None;
                                }
                                code = 0x10000 + ((code - 0xd800) << 10) + low - 0xdc00;
                            }
                            char::from_u32(code)?
                        }
                        _ => return None,
                    });
                }
                c if c < '\u{20}' => return None,
                c => out.push(c),
            }
        }
    }
    fn value(&mut self) -> Option<Value> {
        self.ws();
        if self.0.starts_with('"') {
            return self.string().map(Value::Text);
        }
        if self.eat("true") {
            return Some(Value::Flag(true));
        }
        if self.eat("false") {
            return Some(Value::Flag(false));
        }
        if self.eat("[") {
            let mut values = Vec::new();
            if !self.eat("]") {
                loop {
                    if values.len() == 32 {
                        return None;
                    }
                    values.push(self.string()?);
                    if self.eat("]") {
                        break;
                    }
                    if !self.eat(",") {
                        return None;
                    }
                }
            }
            return Some(Value::Strings(values));
        }
        let end = self
            .0
            .find([',', '}', ' ', '\t', '\r', '\n'])
            .unwrap_or(self.0.len());
        let token = &self.0[..end];
        let digits = token.strip_prefix('-').unwrap_or(token);
        if digits.is_empty()
            || !digits.bytes().all(|b| b.is_ascii_digit())
            || (digits.len() > 1 && digits.starts_with('0'))
        {
            return None;
        }
        let value = token.parse().ok()?;
        self.0 = &self.0[end..];
        Some(Value::Integer(value))
    }
}

pub fn parse(raw: &str) -> Option<Fields> {
    if raw.len() > 16384 {
        return None;
    }
    let mut parser = Parser(raw);
    let mut fields = Fields::default();
    if !parser.eat("{") {
        return None;
    }
    if !parser.eat("}") {
        loop {
            if fields.0.len() == 32 {
                return None;
            }
            let key = parser.string()?;
            if !parser.eat(":") {
                return None;
            }
            let value = parser.value()?;
            let correct_type = match key.as_str() {
                "author" | "message" | "capsule_id" | "event_id" => matches!(value, Value::Text(_)),
                "hz" | "trit" => matches!(value, Value::Integer(_)),
                "triad" => matches!(value, Value::Flag(_)),
                "capsules" => matches!(value, Value::Strings(_)),
                _ => true,
            };
            if !correct_type || fields.0.insert(key, value).is_some() {
                return None;
            }
            if parser.eat("}") {
                break;
            }
            if !parser.eat(",") {
                return None;
            }
        }
    }
    parser.ws();
    parser.0.is_empty().then_some(fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_integer_requests_and_unicode() {
        let fields =
            parse(r#"{"role":"author","author":"Hue\nTokyo\u6771\u4eac\ud83d\udc9a","hz":528}"#)
                .unwrap();
        assert_eq!(fields.text("author"), Some("Hue\nTokyo東京💚"));
        assert_eq!(fields.integer("hz"), Some(528));
        for raw in [
            r#"{"hz":432.5}"#,
            r#"{"hz":4e2}"#,
            r#"{"hz":01}"#,
            r#"{"hz":+1}"#,
            r#"{"hz":"432"}"#,
            r#"{"hz":1,"hz":2}"#,
            r#"{"triad":trueXYZ}"#,
            r#"{"author":"\uD800"}"#,
            r#"{"capsules":["x",]}"#,
            r#"{"hz":9223372036854775808}"#,
            "{}junk",
            "{",
            "{\"x\":{}}",
        ] {
            assert!(parse(raw).is_none(), "{raw}");
        }
    }
}
