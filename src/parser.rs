//! turns JSON text into a [`Value`]. Rendering and mutation live elsewhere.

use crate::{Error, Member, Value, NESTING_LIMIT};

/// parses one complete JSON document — a single value, then only whitespace.
pub fn parse(input: &str) -> Result<Value, Error> {
    let mut parser = Parser::new(input);
    let value = parser.parse_value(0)?;
    parser.skip_whitespace();

    if parser.position != parser.input.len() {
        return Err(parser.error("trailing characters after JSON value"));
    }

    Ok(value)
}

struct Parser<'input> {
    input: &'input [u8],
    position: usize,
}

impl<'input> Parser<'input> {
    fn new(input: &'input str) -> Self {
        Self { input: input.as_bytes(), position: 0 }
    }

    fn error(&self, message: impl Into<String>) -> Error {
        Error::new(self.position, message)
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.input.get(self.position), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.position += 1;
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<Value, Error> {
        if depth >= NESTING_LIMIT {
            return Err(self.error("nesting limit exceeded"));
        }

        self.skip_whitespace();
        match self.input.get(self.position) {
            Some(b'n') => {
                self.consume_literal(b"null")?;
                Ok(Value::Null)
            }
            Some(b't') => {
                self.consume_literal(b"true")?;
                Ok(Value::Bool(true))
            }
            Some(b'f') => {
                self.consume_literal(b"false")?;
                Ok(Value::Bool(false))
            }
            Some(b'"') => self.parse_string().map(Value::String),
            Some(b'[') => self.parse_array(depth + 1),
            Some(b'{') => self.parse_object(depth + 1),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            _ => Err(self.error("expected a JSON value")),
        }
    }

    fn consume_literal(&mut self, literal: &[u8]) -> Result<(), Error> {
        if self.input.get(self.position..self.position + literal.len()) == Some(literal) {
            self.position += literal.len();
            Ok(())
        } else {
            Err(self.error("invalid literal"))
        }
    }

    fn parse_string(&mut self) -> Result<String, Error> {
        debug_assert_eq!(self.input.get(self.position), Some(&b'"'));
        self.position += 1;

        let mut output = String::new();
        loop {
            let byte = *self.input.get(self.position).ok_or_else(|| self.error("unterminated string"))?;
            self.position += 1;

            match byte {
                b'"' => return Ok(output),
                b'\\' => self.parse_escape(&mut output)?,
                0x00..=0x1f => return Err(self.error("unescaped control character in string")),
                _ if byte.is_ascii() => output.push(byte as char),
                _ => self.parse_utf8_character(byte, &mut output)?,
            }
        }
    }

    fn parse_utf8_character(&mut self, first_byte: u8, output: &mut String) -> Result<(), Error> {
        let start = self.position - 1;
        let width = match first_byte {
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => return Err(Error::new(start, "invalid UTF-8 in string")),
        };
        let end = start + width;
        let bytes = self.input.get(start..end).ok_or_else(|| Error::new(start, "incomplete UTF-8 in string"))?;
        let text = std::str::from_utf8(bytes).map_err(|_| Error::new(start, "invalid UTF-8 in string"))?;
        output.push_str(text);
        self.position = end;
        Ok(())
    }

    fn parse_escape(&mut self, output: &mut String) -> Result<(), Error> {
        let escape = *self.input.get(self.position).ok_or_else(|| self.error("incomplete escape"))?;
        self.position += 1;

        match escape {
            b'"' => output.push('"'),
            b'\\' => output.push('\\'),
            b'/' => output.push('/'),
            b'b' => output.push('\u{0008}'),
            b'f' => output.push('\u{000c}'),
            b'n' => output.push('\n'),
            b'r' => output.push('\r'),
            b't' => output.push('\t'),
            b'u' => output.push(self.parse_unicode_escape()?),
            _ => return Err(self.error("invalid string escape")),
        }
        Ok(())
    }

    fn parse_unicode_escape(&mut self) -> Result<char, Error> {
        let first = self.parse_hex_quad()?;
        if (0xd800..=0xdbff).contains(&first) {
            if self.input.get(self.position..self.position + 2) != Some(b"\\u") {
                return Err(self.error("high surrogate is missing its low surrogate"));
            }
            self.position += 2;
            let second = self.parse_hex_quad()?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return Err(self.error("invalid low surrogate"));
            }
            let codepoint = 0x1_0000 + ((first - 0xd800) << 10) + second - 0xdc00;
            return char::from_u32(codepoint).ok_or_else(|| self.error("invalid Unicode scalar"));
        }
        if (0xdc00..=0xdfff).contains(&first) {
            return Err(self.error("unexpected low surrogate"));
        }
        char::from_u32(first).ok_or_else(|| self.error("invalid Unicode scalar"))
    }

    fn parse_hex_quad(&mut self) -> Result<u32, Error> {
        let mut result = 0_u32;
        for _ in 0..4 {
            let byte = *self.input.get(self.position).ok_or_else(|| self.error("incomplete Unicode escape"))?;
            self.position += 1;
            let digit = match byte {
                b'0'..=b'9' => u32::from(byte - b'0'),
                b'a'..=b'f' => u32::from(byte - b'a' + 10),
                b'A'..=b'F' => u32::from(byte - b'A' + 10),
                _ => return Err(self.error("invalid Unicode escape")),
            };
            result = result * 16 + digit;
        }
        Ok(result)
    }

    fn parse_number(&mut self) -> Result<Value, Error> {
        let start = self.position;
        if self.input.get(self.position) == Some(&b'-') {
            self.position += 1;
        }

        match self.input.get(self.position) {
            Some(b'0') => self.position += 1,
            Some(b'1'..=b'9') => {
                self.position += 1;
                self.consume_digits();
            }
            _ => return Err(self.error("invalid number")),
        }

        if self.input.get(self.position) == Some(&b'.') {
            self.position += 1;
            let fraction_start = self.position;
            self.consume_digits();
            if self.position == fraction_start {
                return Err(self.error("fraction requires at least one digit"));
            }
        }

        if matches!(self.input.get(self.position), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(self.input.get(self.position), Some(b'+' | b'-')) {
                self.position += 1;
            }
            let exponent_start = self.position;
            self.consume_digits();
            if self.position == exponent_start {
                return Err(self.error("exponent requires at least one digit"));
            }
        }

        let source = std::str::from_utf8(&self.input[start..self.position]).expect("number tokens are ASCII");
        let number = source.parse::<f64>().map_err(|_| self.error("invalid number"))?;
        if !number.is_finite() {
            return Err(self.error("number is outside the finite f64 range"));
        }
        Ok(Value::Number(number))
    }

    fn consume_digits(&mut self) {
        while matches!(self.input.get(self.position), Some(b'0'..=b'9')) {
            self.position += 1;
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<Value, Error> {
        self.position += 1;
        self.skip_whitespace();
        let mut values = Vec::new();

        if self.input.get(self.position) == Some(&b']') {
            self.position += 1;
            return Ok(Value::Array(values));
        }

        loop {
            values.push(self.parse_value(depth)?);
            self.skip_whitespace();
            match self.input.get(self.position) {
                Some(b',') => {
                    self.position += 1;
                    self.skip_whitespace();
                }
                Some(b']') => {
                    self.position += 1;
                    return Ok(Value::Array(values));
                }
                _ => return Err(self.error("expected ',' or ']' in array")),
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<Value, Error> {
        self.position += 1;
        self.skip_whitespace();
        let mut members = Vec::new();

        if self.input.get(self.position) == Some(&b'}') {
            self.position += 1;
            return Ok(Value::Object(members));
        }

        loop {
            if self.input.get(self.position) != Some(&b'"') {
                return Err(self.error("object key must be a string"));
            }
            let name = self.parse_string()?;
            self.skip_whitespace();
            if self.input.get(self.position) != Some(&b':') {
                return Err(self.error("expected ':' after object key"));
            }
            self.position += 1;
            let value = self.parse_value(depth)?;
            members.push(Member::new(name, value));

            self.skip_whitespace();
            match self.input.get(self.position) {
                Some(b',') => {
                    self.position += 1;
                    self.skip_whitespace();
                }
                Some(b'}') => {
                    self.position += 1;
                    return Ok(Value::Object(members));
                }
                _ => return Err(self.error("expected ',' or '}' in object")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{parse, Member, Value};

    #[test]
    fn parses_all_json_value_kinds() {
        assert_eq!(parse("null"), Ok(Value::Null));
        assert_eq!(parse("false"), Ok(Value::Bool(false)));
        assert_eq!(parse("-12.5e2"), Ok(Value::Number(-1250.0)));
        assert_eq!(parse(r#""text""#), Ok(Value::String("text".into())));
        assert_eq!(parse("[]"), Ok(Value::Array(vec![])));
        assert_eq!(parse("{}"), Ok(Value::Object(vec![])));
    }

    #[test]
    fn preserves_object_order_and_duplicate_keys() {
        let value = parse(r#"{"a":1,"a":2}"#).unwrap();
        assert_eq!(value, Value::object([
            Member::new("a", Value::number(1.0)),
            Member::new("a", Value::number(2.0)),
        ]));
    }

    #[test]
    fn decodes_escapes_and_unicode_surrogate_pairs() {
        assert_eq!(parse(r#""line\n\uD83D\uDE80""#), Ok(Value::string("line\n🚀")));
    }

    #[test]
    fn rejects_invalid_json_and_exposes_an_offset() {
        let error = parse(r#"{"a":01}"#).unwrap_err();
        assert!(error.offset > 0);
        assert!(parse(r#""\uD800""#).is_err());
        assert!(parse("[1,]").is_err());
        assert!(parse("true false").is_err());
    }
}
