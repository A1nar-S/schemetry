use schemetry_mcp::{Command, SERVER_NAME, USAGE, parse_args, run_stdio};

fn main() {
    let command = match parse_args(std::env::args_os().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    match command {
        Command::Help => println!("{USAGE}"),
        Command::Version => println!("{SERVER_NAME} {}", env!("CARGO_PKG_VERSION")),
        Command::Serve { db_override } => {
            if let Err(error) = run_stdio(db_override) {
                eprintln!("{error:#}");
                std::process::exit(1);
            }
        }
    }
}
