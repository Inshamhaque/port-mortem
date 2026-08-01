//! cJSON-compatible text minification.
//!
//! This works on text rather than a parsed [`Value`](crate::Value), just as
//! `cJSON_Minify` does. It intentionally removes `//` and `/* ... */`
//! comments because the C implementation supports that extension.

/// Removes cJSON's insignificant whitespace and comments from JSON-like text.
///
/// Text inside quoted strings is copied unchanged, including whitespace,
/// comment-looking text, and escapes. As in cJSON, a lone slash outside a
/// string is discarded.
pub fn minify(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut characters = input.chars().peekable();

    while let Some(character) = characters.next() {
        match character {
            ' ' | '\t' | '\r' | '\n' => {}
            '"' => copy_string(&mut output, &mut characters),
            '/' => match characters.peek() {
                Some('/') => {
                    characters.next();
                    skip_one_line_comment(&mut characters);
                }
                Some('*') => {
                    characters.next();
                    skip_multiline_comment(&mut characters);
                }
                _ => {}
            },
            character => output.push(character),
        }
    }

    output
}

fn copy_string(output: &mut String, characters: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    output.push('"');
    while let Some(character) = characters.next() {
        output.push(character);
        if character == '"' {
            return;
        }
        if character == '\\' {
            if let Some(escaped) = characters.next() {
                output.push(escaped);
            }
        }
    }
}

fn skip_one_line_comment(characters: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for character in characters.by_ref() {
        if character == '\n' {
            return;
        }
    }
}

fn skip_multiline_comment(characters: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(character) = characters.next() {
        if character == '*' && characters.next_if_eq(&'/').is_some() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::minify;

    #[test]
    fn removes_json_whitespace() {
        assert_eq!(minify("{ \"key\":\ttrue\r\n    }"), "{\"key\":true}");
    }

    #[test]
    fn removes_single_and_multiline_comments() {
        assert_eq!(minify("{// comment\n/* another */\"key\":true}"), "{\"key\":true}");
        assert_eq!(minify("{/* unfinished"), "{");
    }

    #[test]
    fn preserves_strings_and_escaped_quotes() {
        assert_eq!(minify(r#" { "text": " // not a comment \" and spaces " } "#), r#"{"text":" // not a comment \" and spaces "}"#);
    }

    #[test]
    fn preserves_an_unclosed_string() {
        assert_eq!(minify("\"\\"), "\"\\");
    }

    #[test]
    fn mirrors_cjsons_lone_slash_behavior() {
        assert_eq!(minify("8 / 5\n"), "85");
    }
}
