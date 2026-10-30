#![windows_subsystem = "windows"]

use std::ffi::OsString;

fn main() {
    let db_override = match parse_db_override() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };

    if let Err(error) = schemetry_lib::run(db_override) {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn parse_db_override() -> Result<Option<OsString>, String> {
    let mut args = std::env::args_os().skip(1);
    let mut db_override = None;

    while let Some(argument) = args.next() {
        if argument == "--db" {
            if db_override.is_some() {
                return Err("--db may only be specified once".to_string());
            }
            let value = args
                .next()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "--db requires a database path".to_string())?;
            db_override = Some(value);
        } else if let Some(value) = argument
            .to_str()
            .and_then(|argument| argument.strip_prefix("--db="))
        {
            if db_override.is_some() {
                return Err("--db may only be specified once".to_string());
            }
            if value.is_empty() {
                return Err("--db requires a database path".to_string());
            }
            db_override = Some(OsString::from(value));
        }
    }

    Ok(db_override)
}
