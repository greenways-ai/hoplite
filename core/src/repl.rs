use hoplite::hara_source;
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::Editor;
use std::env;
use std::path::PathBuf;

const SPLASH: &str = r#"
 ██╗  ██╗ ██████╗ ██████╗ ██╗     ██╗████████╗███████╗
 ██║  ██║██╔═══██╗██╔══██╗██║     ██║╚══██╔══╝██╔════╝
 ███████║██║   ██║██████╔╝██║     ██║   ██║   █████╗
 ██╔══██║██║   ██║██╔═══╝ ██║     ██║   ██║   ██╔══╝
 ██║  ██║╚██████╔╝██║     ███████╗██║   ██║   ███████╗
 ╚═╝  ╚═╝ ╚═════╝ ╚═╝     ╚══════╝╚═╝   ╚═╝   ╚══════╝
                    HARA ON NGINX
"#;

pub(crate) fn run() -> Result<(), String> {
    let mut runtime = hara_source::compiler_runtime()?;
    super::dev_console::install(&mut runtime);
    let mut editor = Editor::<(), DefaultHistory>::new()
        .map_err(|error| format!("terminal initialization failed: {error}"))?;
    let history = history_file();
    let _ = editor.load_history(&history);
    print_header();

    loop {
        match editor.readline("[hoplite] ") {
            Ok(line) => {
                let source = line.trim();
                if source.is_empty() {
                    continue;
                }
                let _ = editor.add_history_entry(source);
                let _ = editor.save_history(&history);
                if source.starts_with('/') {
                    if !command(source)? {
                        break;
                    }
                } else {
                    match runtime.eval_native(source) {
                        Ok(value) => println!("=> {value}\n"),
                        Err(error) => eprintln!("{error}\n"),
                    }
                }
            }
            Err(ReadlineError::Interrupted) => println!("^C"),
            Err(ReadlineError::Eof) => break,
            Err(error) => return Err(format!("terminal read failed: {error}")),
        }
    }
    Ok(())
}

fn command(source: &str) -> Result<bool, String> {
    let fields = source.split_whitespace().collect::<Vec<_>>();
    match fields.first().copied() {
        Some("/quit" | "/exit") => return Ok(false),
        Some("/help") => print_help(),
        Some("/splash") => print_header(),
        Some("/nginx") => {
            let mut arguments = fields[1..]
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>();
            if arguments.is_empty() {
                arguments.push("status".into());
            }
            super::run_serve_command(&arguments)?;
        }
        Some(command) => return Err(format!("unknown REPL command: {command}")),
        None => {}
    }
    Ok(true)
}

fn print_header() {
    println!("{SPLASH}");
    println!("  /nginx start [PROJECT]    /nginx stop [PROJECT]");
    println!("  /nginx status [PROJECT]   /nginx reload [PROJECT]");
    println!("  /nginx build [PROJECT]    /nginx check [PROJECT]");
    println!("  /help                     /quit\n");
}

fn print_help() {
    println!("Enter Hara forms to evaluate them in the ROOT session.");
    println!("Use /nginx ACTION [PROJECT] to control the packaged Nginx host.");
    println!("Actions: start, stop, reload, status, build, check.\n");
}

fn history_file() -> PathBuf {
    env::var_os("HOPLITE_HISTORY")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".hoplite_history")))
        .unwrap_or_else(|| PathBuf::from(".hoplite_history"))
}

#[cfg(test)]
mod tests {
    use super::{command, SPLASH};

    #[test]
    fn splash_is_hoplite_branded() {
        assert!(SPLASH.contains("HARA ON NGINX"));
        assert!(!SPLASH.contains("JOURNEY WITHIN"));
    }

    #[test]
    fn quit_commands_end_the_repl() {
        assert!(!command("/quit").unwrap());
        assert!(!command("/exit").unwrap());
    }
}
