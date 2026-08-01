//! Turns a [`Value`] back into JSON text, in cJSON's compact or formatted style.

use std::fmt::Write;

use crate::Value;

/// Prints a value with cJSON's tab-indented formatting.
///
/// Returns `None` for `Value::Invalid`, because an invalid node can't be rendered.
pub fn print(value: &Value) -> Option<String> {
    render(value, true)
}

/// Prints a value with no insignificant whitespace.
pub fn print_unformatted(value: &Value) -> Option<String> {
    render(value, false)
}

fn render(value: &Value, formatted: bool) -> Option<String> {
    let mut output = String::new();
    write_value(&mut output, value, formatted, 0)?;
    Some(output)
}

fn write_value(output: &mut String, value: &Value, formatted: bool, depth: usize) -> Option<()> {
    match value {
        Value::Invalid => return None,
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&format_number(*value)),
        Value::String(value) => write_string(output, value),
        Value::Raw(value) => output.push_str(value),
        Value::Array(values) => write_array(output, values, formatted, depth)?,
        Value::Object(members) => write_object(output, members, formatted, depth)?,
    }
    Some(())
}

fn write_array(output: &mut String, values: &[Value], formatted: bool, depth: usize) -> Option<()> {
    output.push('[');
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            output.push(',');
            if formatted {
                output.push(' ');
            }
        }
        write_value(output, value, formatted, depth + 1)?;
    }
    output.push(']');
    Some(())
}

fn write_object(
    output: &mut String,
    members: &[crate::Member],
    formatted: bool,
    depth: usize,
) -> Option<()> {
    output.push('{');
    if formatted {
        output.push('\n');
    }

    for (index, member) in members.iter().enumerate() {
        if formatted {
            write_indent(output, depth + 1);
        }
        write_string(output, &member.name);
        output.push(':');
        if formatted {
            output.push('\t');
        }
        write_value(output, &member.value, formatted, depth + 1)?;
        if index + 1 != members.len() {
            output.push(',');
        }
        if formatted {
            output.push('\n');
        }
    }

    if formatted {
        write_indent(output, depth);
    }
    output.push('}');
    Some(())
}

fn write_indent(output: &mut String, depth: usize) {
    for _ in 0..depth {
        output.push('\t');
    }
}

fn write_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{0008}' => output.push_str("\\b"),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\u{000c}' => output.push_str("\\f"),
            '\r' => output.push_str("\\r"),
            character if character <= '\u{001f}' => {
                write!(output, "\\u{:04x}", character as u32).expect("writing to String cannot fail");
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

/// Formats a number the way cJSON does: try 15 significant digits first, then
/// bump to 17 if that shorter form would lose precision.
fn format_number(value: f64) -> String {
    if !value.is_finite() {
        return "null".into();
    }
    if value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX) && value == value.trunc() {
        return (value as i32).to_string();
    }

    let short = format_significant(value, 15);
    let reparsed = short.parse::<f64>().ok();
    if reparsed.is_some_and(|candidate| close_enough(candidate, value)) {
        short
    } else {
        format_significant(value, 17)
    }
}

fn close_enough(left: f64, right: f64) -> bool {
    (left - right).abs() <= left.abs().max(right.abs()) * f64::EPSILON
}

fn format_significant(value: f64, precision: usize) -> String {
    let exponent = value.abs().log10().floor() as i32;
    if exponent < -4 || exponent >= precision as i32 {
        let scientific = format!("{:.*e}", precision - 1, value);
        return normalize_scientific(&trim_fractional_zeros(scientific));
    }

    let decimals = (precision as i32 - exponent - 1).max(0) as usize;
    trim_fractional_zeros(format!("{:.*}", decimals, value))
}

fn trim_fractional_zeros(mut text: String) -> String {
    let Some(decimal_point) = text.find('.') else { return text; };
    let exponent = text.find(['e', 'E']).unwrap_or(text.len());
    let mut end = exponent;
    while end > decimal_point + 1 && text.as_bytes()[end - 1] == b'0' {
        end -= 1;
    }
    if end == decimal_point + 1 {
        end -= 1;
    }
    if end != exponent {
        text.replace_range(end..exponent, "");
    }
    text
}

fn normalize_scientific(text: &str) -> String {
    let (mantissa, exponent) = text.split_once('e').expect("scientific formatting contains e");
    let exponent = exponent.parse::<i32>().expect("Rust produced a valid exponent");
    format!("{mantissa}e{exponent:+03}")
}

#[cfg(test)]
mod tests {
    use crate::{parse, print, print_unformatted, Value};

    #[test]
    fn writes_compact_json() {
        let value = parse(r#"{"items":[1,true,"x"]}"#).unwrap();
        assert_eq!(print_unformatted(&value), Some(r#"{"items":[1,true,"x"]}"#.into()));
    }

    #[test]
    fn writes_cjson_style_formatted_objects_and_arrays() {
        let value = parse(r#"{"one":1,"nested":{}}"#).unwrap();
        assert_eq!(print(&value), Some("{\n\t\"one\":\t1,\n\t\"nested\":\t{\n\t}\n}".into()));
        assert_eq!(print(&Value::array([Value::number(1.0), Value::number(2.0)])), Some("[1, 2]".into()));
    }

    #[test]
    fn escapes_control_characters_and_keeps_utf8() {
        assert_eq!(print_unformatted(&Value::string("\u{0001}\nü")), Some(r#""\u0001\nü""#.into()));
    }

    #[test]
    fn formats_numbers_like_cjson() {
        assert_eq!(print_unformatted(&Value::number(1e-9)), Some("1e-09".into()));
        assert_eq!(print_unformatted(&Value::number(3.141592653589793)), Some("3.1415926535897931".into()));
        assert_eq!(print_unformatted(&Value::number(f64::NAN)), Some("null".into()));
    }

    #[test]
    fn rejects_invalid_nodes() {
        assert_eq!(print(&Value::Invalid), None);
    }
}
