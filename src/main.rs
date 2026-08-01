use std::env;
use std::io::Read;
use std::process::ExitCode;

use cjson_rs::{apply_patches, get_pointer, minify, parse, print, print_unformatted};

const USAGE: &str = "\
cjson-rs <command> [options] [file]

Reads JSON from a file, or from stdin when 'file' is '-' or omitted.

Commands:
  parse     Validate and print the document (formatted)
  print     Print the document (--format for tab-indented, default compact)
  minify    Strip insignificant whitespace and comments
  get       Print the value at a JSON Pointer, or error if it does not resolve
  patch     Apply a JSON Patch from the second file

Examples:
  cjson-rs parse doc.json
  cjson-rs minify - < doc.json
  cjson-rs get doc.json /a/b/0
  cjson-rs patch doc.json patch.json
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(command) = args.first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };

    if matches!(command.as_str(), "help" | "--help" | "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let result = match command.as_str() {
        "parse" => run(&args[1..], cmd_parse),
        "print" => run(&args[1..], cmd_print),
        "minify" => run(&args[1..], cmd_minify),
        "get" => run(&args[1..], cmd_get),
        "patch" => run(&args[1..], cmd_patch),
        other => {
            eprintln!("unknown command: {other}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String], command: impl FnOnce(&[String]) -> Result<String, String>) -> Result<String, String> {
    command(args)
}

fn cmd_parse(args: &[String]) -> Result<String, String> {
    let input = read_input(args)?;
    let value = parse(&input).map_err(|error| error.to_string())?;
    print(&value).ok_or_else(|| "cannot print an invalid value".to_string())
}

fn cmd_print(args: &[String]) -> Result<String, String> {
    let formatted = matches!(args.first().map(String::as_str), Some("--format"));
    let positional = if formatted { &args[1..] } else { args };
    let input = read_input(positional)?;
    let value = parse(&input).map_err(|error| error.to_string())?;
    if formatted {
        print(&value).ok_or_else(|| "cannot print an invalid value".to_string())
    } else {
        print_unformatted(&value).ok_or_else(|| "cannot print an invalid value".to_string())
    }
}

fn cmd_minify(args: &[String]) -> Result<String, String> {
    let input = read_input(args)?;
    Ok(minify(&input))
}

fn cmd_get(args: &[String]) -> Result<String, String> {
    let (file, pointer) = split_two(args, "usage: cjson-rs get <file> <pointer>")?;
    let input = read_file(&file)?;
    let value = parse(&input).map_err(|error| error.to_string())?;
    match get_pointer(&value, &pointer, false) {
        Some(found) => print_unformatted(found).ok_or_else(|| "cannot print an invalid value".to_string()),
        None => Err(format!("pointer {pointer:?} does not resolve")),
    }
}

fn cmd_patch(args: &[String]) -> Result<String, String> {
    let (file, patch_file) = split_two(args, "usage: cjson-rs patch <file> <patch-file>")?;
    let input = read_file(&file)?;
    let patches_input = read_file(&patch_file)?;
    let mut value = parse(&input).map_err(|error| error.to_string())?;
    let patches = parse(&patches_input).map_err(|error| error.to_string())?;
    apply_patches(&mut value, &patches, false).map_err(|status| format!("patch failed with status {}", status.0))?;
    print_unformatted(&value).ok_or_else(|| "cannot print an invalid value".to_string())
}

fn read_input(args: &[String]) -> Result<String, String> {
    match args.first().map(String::as_str) {
        None | Some("-") => read_stdin(),
        Some(path) => read_file(path),
    }
}

fn read_file(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| format!("cannot read {path}: {error}"))
}

fn read_stdin() -> Result<String, String> {
    let mut buffer = String::new();
    std::io::stdin().read_to_string(&mut buffer).map_err(|error| format!("cannot read stdin: {error}"))?;
    Ok(buffer)
}

fn split_two(args: &[String], usage: &str) -> Result<(String, String), String> {
    if args.len() != 2 {
        return Err(usage.to_string());
    }
    Ok((args[0].clone(), args[1].clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_prints_formatted_json() {
        let input = temp_file(r#"{"a":1}"#);
        let output = cmd_parse(&[input.clone()]).unwrap();
        assert_eq!(output, "{\n\t\"a\":\t1\n}");
        cleanup(input);
    }

    #[test]
    fn print_command_switches_between_styles() {
        let input = temp_file(r#"{"a":[1,2]}"#);
        let compact = cmd_print(&[input.clone()]).unwrap();
        assert_eq!(compact, r#"{"a":[1,2]}"#);
        let formatted = cmd_print(&["--format".into(), input.clone()]).unwrap();
        assert_eq!(formatted, "{\n\t\"a\":\t[1, 2]\n}");
        cleanup(input);
    }

    #[test]
    fn minify_command_removes_whitespace() {
        let input = temp_file("{ \"a\" : true }");
        let output = cmd_minify(&[input.clone()]).unwrap();
        assert_eq!(output, r#"{"a":true}"#);
        cleanup(input);
    }

    #[test]
    fn get_command_resolves_and_reports_missing_pointers() {
        let input = temp_file(r#"{"a":{"b":5}}"#);
        let found = cmd_get(&[input.clone(), "/a/b".into()]).unwrap();
        assert_eq!(found, "5");
        let missing = cmd_get(&[input.clone(), "/nope".into()]).unwrap_err();
        assert!(missing.contains("does not resolve"), "got: {missing}");
        cleanup(input);
    }

    #[test]
    fn patch_command_applies_a_patch_file() {
        let input = temp_file(r#"{"a":1}"#);
        let patch_file = temp_file(r#"[{"op":"replace","path":"/a","value":9}]"#);
        let output = cmd_patch(&[input.clone(), patch_file.clone()]).unwrap();
        assert_eq!(output, r#"{"a":9}"#);
        cleanup(input);
        cleanup(patch_file);
    }

    fn temp_file(contents: &str) -> String {
        let path = std::env::temp_dir().join(format!("cjson_rs_test_{}_{}", std::process::id(), rand()));
        std::fs::write(&path, contents).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn rand() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos() as u64
    }

    fn cleanup(path: String) {
        let _ = std::fs::remove_file(path);
    }
}
