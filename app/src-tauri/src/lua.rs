//! Minimal parser for World of Warcraft SavedVariables files (Lua table literals).

use anyhow::{anyhow, bail, Result};
use serde_json::{Map, Number, Value};

pub fn parse_assignments(source: &str) -> Result<Map<String, Value>> {
    let mut parser = Parser { bytes: source.as_bytes(), position: 0 };
    let mut globals = Map::new();
    loop {
        parser.skip_trivia();
        if parser.at_end() {
            return Ok(globals);
        }
        let name = parser.identifier().ok_or_else(|| parser.error("expected a variable name"))?;
        parser.skip_trivia();
        parser.expect(b'=')?;
        let value = parser.value()?;
        globals.insert(name, value);
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
}

enum Key {
    Index(i64),
    Name(String),
}

impl Parser<'_> {
    fn at_end(&self) -> bool {
        self.position >= self.bytes.len()
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn rest(&self) -> &[u8] {
        &self.bytes[self.position.min(self.bytes.len())..]
    }

    fn error(&self, message: &str) -> anyhow::Error {
        anyhow!("{message} at byte {}", self.position)
    }

    fn expect(&mut self, byte: u8) -> Result<()> {
        if self.peek() == Some(byte) {
            self.position += 1;
            Ok(())
        } else {
            Err(self.error(&format!("expected '{}'", byte as char)))
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
                self.position += 1;
            }
            if self.rest().starts_with(b"--") {
                self.position += 2;
                if let Some(level) = self.long_bracket_level() {
                    self.long_bracket(level);
                } else {
                    while self.peek().is_some_and(|byte| byte != b'\n') {
                        self.position += 1;
                    }
                }
                continue;
            }
            if self.rest().starts_with(&[0xEF, 0xBB, 0xBF]) {
                self.position += 3;
                continue;
            }
            return;
        }
    }

    fn long_bracket_level(&self) -> Option<usize> {
        let rest = self.rest();
        if rest.first() != Some(&b'[') {
            return None;
        }
        let equals = rest[1..].iter().take_while(|byte| **byte == b'=').count();
        (rest.get(1 + equals) == Some(&b'[')).then_some(equals)
    }

    fn long_bracket(&mut self, level: usize) -> String {
        self.position += level + 2;
        let start = self.position;
        let mut closing = vec![b']'];
        closing.extend(std::iter::repeat_n(b'=', level));
        closing.push(b']');
        while !self.at_end() && !self.rest().starts_with(&closing) {
            self.position += 1;
        }
        let end = self.position.min(self.bytes.len());
        let text = String::from_utf8_lossy(&self.bytes[start..end]).into_owned();
        self.position = (self.position + closing.len()).min(self.bytes.len());
        text
    }

    fn identifier(&mut self) -> Option<String> {
        let start = self.position;
        if !self.peek().is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_') {
            return None;
        }
        while self.peek().is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
            self.position += 1;
        }
        Some(String::from_utf8_lossy(&self.bytes[start..self.position]).into_owned())
    }

    fn value(&mut self) -> Result<Value> {
        self.skip_trivia();
        match self.peek() {
            Some(b'{') => self.table(),
            Some(b'"') | Some(b'\'') => Ok(Value::String(self.string()?)),
            Some(b'[') => match self.long_bracket_level() {
                Some(level) => Ok(Value::String(self.long_bracket(level))),
                None => Err(self.error("unexpected '['")),
            },
            Some(byte) if byte == b'-' || byte == b'.' || byte.is_ascii_digit() => self.number(),
            Some(_) => {
                let word = self.identifier().ok_or_else(|| self.error("unexpected character"))?;
                Ok(match word.as_str() {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    _ => Value::Null,
                })
            }
            None => bail!("unexpected end of file"),
        }
    }

    fn number(&mut self) -> Result<Value> {
        let start = self.position;
        while self.peek().is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'+' | b'.' | b'(' | b')')) {
            self.position += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.position])?;
        if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            return Ok(i64::from_str_radix(hex, 16).map(Value::from).unwrap_or(Value::Null));
        }
        if let Ok(integer) = text.parse::<i64>() {
            return Ok(Value::from(integer));
        }
        Ok(text.parse::<f64>().ok().and_then(Number::from_f64).map(Value::Number).unwrap_or(Value::Null))
    }

    fn string(&mut self) -> Result<String> {
        let quote = self.peek().unwrap_or(b'"');
        self.position += 1;
        let mut output = Vec::new();
        loop {
            let byte = self.peek().ok_or_else(|| self.error("unterminated string"))?;
            self.position += 1;
            if byte == quote {
                break;
            }
            if byte != b'\\' {
                output.push(byte);
                continue;
            }
            let escaped = self.peek().ok_or_else(|| self.error("unterminated escape"))?;
            self.position += 1;
            match escaped {
                b'n' | b'\n' => output.push(b'\n'),
                b'\r' => {
                    if self.peek() == Some(b'\n') {
                        self.position += 1;
                    }
                    output.push(b'\n');
                }
                b't' => output.push(b'\t'),
                b'r' => output.push(b'\r'),
                b'a' => output.push(7),
                b'b' => output.push(8),
                b'f' => output.push(12),
                b'v' => output.push(11),
                b'0'..=b'9' => {
                    let mut code = u32::from(escaped - b'0');
                    for _ in 0..2 {
                        match self.peek() {
                            Some(digit @ b'0'..=b'9') => {
                                code = code * 10 + u32::from(digit - b'0');
                                self.position += 1;
                            }
                            _ => break,
                        }
                    }
                    output.push(code.min(255) as u8);
                }
                other => output.push(other),
            }
        }
        Ok(String::from_utf8_lossy(&output).into_owned())
    }

    fn table(&mut self) -> Result<Value> {
        self.expect(b'{')?;
        let mut entries: Vec<(Key, Value)> = Vec::new();
        let mut next_index = 1i64;
        loop {
            self.skip_trivia();
            match self.peek() {
                Some(b'}') => {
                    self.position += 1;
                    break;
                }
                Some(b',') | Some(b';') => {
                    self.position += 1;
                    continue;
                }
                None => bail!("unterminated table"),
                _ => {}
            }
            if self.peek() == Some(b'[') && self.long_bracket_level().is_none() {
                self.position += 1;
                let key = self.value()?;
                self.skip_trivia();
                self.expect(b']')?;
                self.skip_trivia();
                self.expect(b'=')?;
                let value = self.value()?;
                let key = match key {
                    Value::Number(number) if number.as_i64().is_some() => Key::Index(number.as_i64().unwrap_or_default()),
                    Value::String(text) => Key::Name(text),
                    other => Key::Name(other.to_string()),
                };
                entries.push((key, value));
                continue;
            }
            let save = self.position;
            if let Some(name) = self.identifier() {
                self.skip_trivia();
                if self.peek() == Some(b'=') {
                    self.position += 1;
                    let value = self.value()?;
                    entries.push((Key::Name(name), value));
                    continue;
                }
                self.position = save;
            }
            let value = self.value()?;
            entries.push((Key::Index(next_index), value));
            next_index += 1;
        }
        Ok(build_table(entries))
    }
}

fn build_table(entries: Vec<(Key, Value)>) -> Value {
    if entries.is_empty() {
        return Value::Array(Vec::new());
    }
    if entries.iter().all(|(key, _)| matches!(key, Key::Index(_))) {
        let mut indexed: Vec<(i64, Value)> = entries
            .into_iter()
            .filter_map(|(key, value)| match key {
                Key::Index(index) => Some((index, value)),
                Key::Name(_) => None,
            })
            .collect();
        indexed.sort_by_key(|(index, _)| *index);
        let consecutive = indexed.iter().enumerate().all(|(position, (index, _))| *index == position as i64 + 1);
        if consecutive {
            return Value::Array(indexed.into_iter().map(|(_, value)| value).collect());
        }
        return Value::Object(indexed.into_iter().map(|(index, value)| (index.to_string(), value)).collect());
    }
    Value::Object(
        entries
            .into_iter()
            .map(|(key, value)| match key {
                Key::Index(index) => (index.to_string(), value),
                Key::Name(name) => (name, value),
            })
            .collect(),
    )
}

pub fn escape_string(text: &str) -> String {
    let mut output = String::with_capacity(text.len() + 2);
    output.push('"');
    for character in text.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            control if (control as u32) < 32 => output.push_str(&format!("\\{:03}", control as u32)),
            other => output.push(other),
        }
    }
    output.push('"');
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_saved_variables() {
        let source = "\nMentorDB = {\n[\"last\"] = \"Jeanne-Ocre\",\n[\"list\"] = {\n\"a\", -- [1]\n\"b\\\"c\", -- [2]\n},\n[\"n\"] = -1.5,\n[\"ok\"] = true,\n}\nOther = nil\n";
        let globals = parse_assignments(source).unwrap();
        let db = &globals["MentorDB"];
        assert_eq!(db["last"], "Jeanne-Ocre");
        assert_eq!(db["list"][1], "b\"c");
        assert_eq!(db["n"], -1.5);
        assert_eq!(db["ok"], true);
        assert!(globals["Other"].is_null());
    }

    #[test]
    fn reads_escaped_windows_newlines() {
        let globals = parse_assignments("X = \"a\\\r\nb\"").unwrap();
        assert_eq!(globals["X"], "a\nb");
    }

    #[test]
    fn escapes_round_trip() {
        let original = "a\"b\\c\nd é";
        let source = format!("X = {}", escape_string(original));
        let globals = parse_assignments(&source).unwrap();
        assert_eq!(globals["X"], original);
    }
}
