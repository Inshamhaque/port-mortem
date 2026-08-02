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
        // cJSON's buffer_skip_whitespace uses isspace(3), whose set is " \t\n\v\f\r".
        // Reproduce that exactly so the safe core and the FFI layer (and the
        // original C) accept the same documents.
        while matches!(self.input.get(self.position), Some(b' ' | b'\n' | b'\r' | b'\t' | b'\x0b' | b'\x0c')) {
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
                // cJSON copies any non-backslash byte through verbatim, including
                // raw control characters (RFC 8259 requires them escaped, but the
                // port keeps cJSON's permissiveness for behavioral parity — see
                // DECISIONS.md D16).
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

        // cJSON copies every byte from [0-9 + - e E .] into a scratch buffer
        // and hands it to strtod, which is far more permissive than RFC 8259:
        // leading zeros ("01" -> 1), a bare fraction ("1." -> 1), and an
        // exponent without digits ("1e" -> 1, consuming only the "1") all
        // parse. The parser advances by whatever strtod consumed, not the full
        // scan. This mirrors parse_c_float in the FFI layer so the safe core
        // and the C-ABI layer accept the same documents (see DECISIONS.md D16).
        let mut scan = start;
        while matches!(self.input.get(scan), Some(b'0'..=b'9' | b'+' | b'-' | b'e' | b'E' | b'.')) {
            scan += 1;
        }
        let (value, consumed) = parse_c_float(&self.input[start..scan])
            .ok_or_else(|| self.error("invalid number"))?;
        self.position = start + consumed;
        Ok(Value::Number(value))
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

/// The subset of C `strtod` that `parse_number` can produce: optional sign,
/// digit/dot mantissa, optional exponent. Returns `(value, bytes consumed)`.
/// Mirrors `parse_c_float` in the FFI layer so both parsers agree. `strtod`
/// leaves a bare exponent marker unconsumed ("1e" -> (1.0, 1)).
fn parse_c_float(input: &[u8]) -> Option<(f64, usize)> {
    let mut pos = 0usize;
    if matches!(input.get(pos), Some(b'+' | b'-')) {
        pos += 1;
    }
    let mut digits = 0usize;
    while matches!(input.get(pos), Some(b'0'..=b'9')) {
        pos += 1;
        digits += 1;
    }
    let mut fraction = 0usize;
    if input.get(pos) == Some(&b'.') {
        pos += 1;
        while matches!(input.get(pos), Some(b'0'..=b'9')) {
            pos += 1;
            fraction += 1;
        }
    }
    if digits == 0 && fraction == 0 {
        return None; // strtod does not advance: parse error
    }
    if matches!(input.get(pos), Some(b'e' | b'E')) {
        let exponent_start = pos;
        pos += 1;
        if matches!(input.get(pos), Some(b'+' | b'-')) {
            pos += 1;
        }
        let exponent_digits = pos;
        while matches!(input.get(pos), Some(b'0'..=b'9')) {
            pos += 1;
        }
        if pos == exponent_digits {
            pos = exponent_start;
        }
    }
    let token = core::str::from_utf8(&input[..pos]).ok()?;
    let value = token.parse::<f64>().ok()?;
    Some((value, pos))
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
        // .5 and --1 are not value starts / valid numbers even in cJSON
        // (parse_value only enters a number on '-' or a digit).
        let error = parse(r#"{"a":.5}"#).unwrap_err();
        assert!(error.offset > 0);
        assert!(parse(r#""\uD800""#).is_err());
        assert!(parse("[1,]").is_err());
        assert!(parse("true false").is_err());
    }

    #[test]
    fn numbers_are_as_permissive_as_cjson() {
        // Leading zeros and bare fractions parse via the strtod-like path,
        // matching cJSON (DECISIONS.md D16).
        assert_eq!(parse("01"), Ok(Value::Number(1.0)));
        assert_eq!(parse("1."), Ok(Value::Number(1.0)));
        // A bare exponent consumes only the "1" (strtod leaves "e" unconsumed),
        // so as a standalone document it is rejected by the whole-input
        // requirement — the same outcome as require_null_terminated=1.
        assert!(parse("1e").is_err());
    }
}
