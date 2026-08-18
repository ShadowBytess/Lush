use lush::{parse, Node, Redirect as AstRedirect, SimpleCommand};
use rustyline::completion::FilenameCompleter;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::HistoryHinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{Completer, Editor, Helper, Hinter};
use std::borrow::Cow;
use std::collections::HashMap;
use std::env;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

#[derive(Helper, Completer, Hinter)]
struct LushHelper {
    #[rustyline(Completer)]
    completer: FilenameCompleter,
    #[rustyline(Hinter)]
    hinter: HistoryHinter,
}

impl Validator for LushHelper {}

impl Highlighter for LushHelper {
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Owned(format!("\x1b[2m{}\x1b[0m", hint))
    }
}

fn main() {
    load_rc();

    let mut rl = Editor::<LushHelper, DefaultHistory>::new()
        .expect("lush: failed to initialize line editor");
    rl.set_helper(Some(LushHelper {
        completer: FilenameCompleter::new(),
        hinter: HistoryHinter::new(),
    }));

    let history_path = history_file_path();
    if let Some(ref path) = history_path {
        let _ = rl.load_history(path);
    }

    loop {
        let prompt = build_prompt();
        match rl.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(line);
                execute_chain(line);
            }
            Err(ReadlineError::Interrupted) => {
                continue;
            }
            Err(ReadlineError::Eof) => {
                break;
            }
            Err(err) => {
                eprintln!("lush: readline error: {}", err);
                break;
            }
        }
    }

    if let Some(ref path) = history_path {
        let _ = rl.save_history(path);
    }
}

fn history_file_path() -> Option<String> {
    env::var("HOME").ok().map(|home| format!("{}/.lush_history", home))
}

fn build_prompt() -> String {
    let cwd = env::current_dir().unwrap_or_else(|_| "/".into());
    let cwd_str = cwd.to_string_lossy().to_string();
    let home = env::var("HOME").unwrap_or_default();

    let display_path = if !home.is_empty() && cwd_str == home {
        "~".to_string()
    } else if !home.is_empty() && cwd_str.starts_with(&format!("{}/", home)) {
        format!("~{}", &cwd_str[home.len()..])
    } else {
        cwd_str
    };

    format!("{}: {}@{}\n⟩ ", display_path, get_username(), get_hostname())
}

fn get_username() -> String {
    env::var("USER")
        .or_else(|_| env::var("LOGNAME"))
        .unwrap_or_else(|_| "user".to_string())
}

fn get_hostname() -> String {
    if let Ok(contents) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let trimmed = contents.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Ok(output) = Command::new("hostname").output() {
        if output.status.success() {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }
    "localhost".to_string()
}

fn load_rc() {
    load_function_files();
    if let Ok(home) = env::var("HOME") {
        let path = format!("{}/.lushrc", home);
        run_script_file(&path, false);
    }
}

fn load_function_files() {
    let dir = match env::var("HOME") {
        Ok(home) => format!("{}/.config/lush/functions", home),
        Err(_) => return,
    };

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("lush") {
            continue;
        }
        let filename_stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("lush: functions: {}: {}", path.display(), e);
                continue;
            }
        };

        let (name, value) = match parse_function_block(&contents) {
            Some((name, value)) => {
                if name != filename_stem {
                    eprintln!(
                        "lush: functions: warning: {} defines '{}', but filename suggests '{}'",
                        path.display(),
                        name,
                        filename_stem
                    );
                }
                (name, value)
            }
            None => {
                let value: String = contents
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .collect::<Vec<_>>()
                    .join(" ");
                if value.is_empty() {
                    eprintln!("lush: functions: {}: file has no content", path.display());
                    continue;
                }
                (filename_stem, value)
            }
        };

        get_aliases().lock().unwrap().insert(name, value);
    }
}

fn parse_function_block(contents: &str) -> Option<(String, String)> {
    let lines: Vec<&str> = contents.lines().collect();

    let mut idx = 0;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            idx += 1;
            continue;
        }
        break;
    }
    if idx >= lines.len() {
        return None;
    }

    let header = lines[idx].trim();
    if !header.starts_with("function ") && header != "function" {
        return None; // no header, let the caller try the legacy format
    }

    let header_tokens = tokenize(header);
    let name = header_tokens.get(1)?.clone();

    let mut body_lines = Vec::new();
    let mut found_end = false;
    idx += 1;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if trimmed == "end" {
            found_end = true;
            break;
        }
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            body_lines.push(trimmed.to_string());
        }
        idx += 1;
    }

    if !found_end {
        eprintln!(
            "lush: functions: warning: function '{}' has no matching 'end'",
            name
        );
    }
    if body_lines.is_empty() {
        eprintln!("lush: functions: warning: function '{}' has an empty body", name);
        return None;
    }

    Some((name, body_lines.join(" ")))
}

fn run_script_file(path: &str, warn_if_missing: bool) -> i32 {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                execute_chain(line);
            }
            0
        }
        Err(e) => {
            if warn_if_missing {
                eprintln!("lush: {}: {}", path, e);
            }
            1
        }
    }
}

fn execute_chain(input: &str) {
    match parse(input) {
        Ok(Some(node)) => {
            exec_node(&node);
        }
        Ok(None) => {} // empty input, nothing to run
        Err(e) => eprintln!("lush: {}", e),
    }
}

fn exec_node(node: &Node) -> i32 {
    match node {
        Node::Command(cmd) => exec_simple_command(cmd),
        Node::Pipeline(cmds) => exec_pipeline(cmds),
        Node::And(lhs, rhs) => {
            let status = exec_node(lhs);
            if status == 0 {
                exec_node(rhs)
            } else {
                status
            }
        }
        Node::Or(lhs, rhs) => {
            let status = exec_node(lhs);
            if status != 0 {
                exec_node(rhs)
            } else {
                status
            }
        }
        Node::Sequence(lhs, rhs) => {
            exec_node(lhs);
            exec_node(rhs)
        }
    }
}

fn redirects_from_ast(redirects: &[AstRedirect]) -> Redirects {
    let mut r = Redirects::default();
    for redirect in redirects {
        match redirect {
            AstRedirect::In(path) => r.stdin = Some(path.clone()),
            AstRedirect::Out(path) => {
                r.stdout = Some(path.clone());
                r.append = false;
            }
            AstRedirect::Append(path) => {
                r.stdout = Some(path.clone());
                r.append = true;
            }
        }
    }
    r
}

fn exec_simple_command(cmd: &SimpleCommand) -> i32 {
    let words = expand_alias(cmd.words.clone());
    if words.is_empty() {
        eprintln!("lush: syntax error: missing command");
        return 1;
    }

    let cmd_name = words[0].as_str();
    let args = &words[1..];

    if args.is_empty() && looks_like_path_token(cmd_name) {
        let expanded = expand_tilde(cmd_name);
        if Path::new(&expanded).is_dir() {
            return run_builtin("cd", &[cmd_name.to_string()]);
        }
    }

    match cmd_name {
        "exit" => std::process::exit(0),
        _ if is_builtin(cmd_name) => run_builtin(cmd_name, args),
        _ => {
            let redirects = redirects_from_ast(&cmd.redirects);
            let stdin = match open_redirect_stdin(&redirects) {
                Some(s) => s,
                None => return 1, // error already printed
            };
            let stdout = match open_redirect_stdout(&redirects) {
                Some(s) => s,
                None => return 1,
            };

            match Command::new(cmd_name).args(args).stdin(stdin).stdout(stdout).spawn() {
                Ok(mut child) => match child.wait() {
                    Ok(status) => status.code().unwrap_or(1),
                    Err(_) => 1,
                },
                Err(e) => {
                    eprintln!("lush: {}: {}", cmd_name, e);
                    127
                }
            }
        }
    }
}

fn get_aliases() -> &'static Mutex<HashMap<String, String>> {
    static ALIASES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    ALIASES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn expand_alias(tokens: Vec<String>) -> Vec<String> {
    if tokens.is_empty() {
        return tokens;
    }
    let expansion = get_aliases().lock().unwrap().get(&tokens[0]).cloned();
    match expansion {
        Some(value) => {
            let mut expanded = tokenize(&value);
            let rest = &tokens[1..];
            match expanded.iter().position(|t| t == "$argv") {
                Some(pos) => {
                    expanded.splice(pos..pos + 1, rest.iter().cloned());
                }
                None => expanded.extend_from_slice(rest),
            }
            expanded
        }
        None => tokens,
    }
}

fn expand_tilde(path: &str) -> String {
    let home = || env::var("HOME").unwrap_or_else(|_| "/".to_string());
    if path == "~" {
        home()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("{}/{}", home(), rest)
    } else {
        path.to_string()
    }
}

fn looks_like_path_token(token: &str) -> bool {
    token == ".." || token == "." || token.starts_with('~') || token.contains('/')
}

fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote_char: Option<char> = None;

    for c in input.chars() {
        if let Some(qc) = quote_char {
            if c == qc {
                quote_char = None;
            } else {
                current.push(c);
            }
        } else if c == '"' || c == '\'' {
            quote_char = Some(c);
        } else if c == ' ' {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

#[derive(Default)]
struct Redirects {
    stdin: Option<String>,
    stdout: Option<String>,
    append: bool, // true for >>, false for >
}

fn open_redirect_stdin(redirects: &Redirects) -> Option<Stdio> {
    match &redirects.stdin {
        None => Some(Stdio::inherit()), // caller may override this for pipelines
        Some(path) => match File::open(path) {
            Ok(f) => Some(Stdio::from(f)),
            Err(e) => {
                eprintln!("lush: {}: {}", path, e);
                None
            }
        },
    }
}

fn open_redirect_stdout(redirects: &Redirects) -> Option<Stdio> {
    match &redirects.stdout {
        None => Some(Stdio::inherit()), // caller may override this for pipelines
        Some(path) => {
            let result = OpenOptions::new()
                .write(true)
                .create(true)
                .append(redirects.append)
                .truncate(!redirects.append)
                .open(path);
            match result {
                Ok(f) => Some(Stdio::from(f)),
                Err(e) => {
                    eprintln!("lush: {}: {}", path, e);
                    None
                }
            }
        }
    }
}

fn is_builtin(cmd: &str) -> bool {
    matches!(cmd, "cd" | "exit" | "alias" | "unalias" | "source" | "." | "funcsave")
}

fn run_builtin(cmd: &str, args: &[String]) -> i32 {
    match cmd {
        "cd" => {
            let target = args.get(0).map(|s| s.as_str()).unwrap_or("~");
            let path = expand_tilde(target);
            match env::set_current_dir(Path::new(&path)) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("lush: cd: {}: {}", path, e);
                    1
                }
            }
        }
        "alias" => {
            if args.is_empty() {
                let aliases = get_aliases().lock().unwrap();
                let mut names: Vec<&String> = aliases.keys().collect();
                names.sort();
                for name in names {
                    println!("alias {}='{}'", name, aliases[name]);
                }
                return 0;
            }

            let full = args.join(" ");
            match full.find('=') {
                Some(eq_idx) => {
                    let name = full[..eq_idx].trim().to_string();
                    let value = full[eq_idx + 1..].trim().to_string();
                    if name.is_empty() {
                        eprintln!("lush: alias: invalid syntax, expected name=value");
                        1
                    } else {
                        get_aliases().lock().unwrap().insert(name, value);
                        0
                    }
                }
                None => {
                    eprintln!("lush: alias: invalid syntax, expected name=value");
                    1
                }
            }
        }
        "unalias" => match args.get(0) {
            Some(name) => {
                if get_aliases().lock().unwrap().remove(name).is_some() {
                    0
                } else {
                    eprintln!("lush: unalias: {}: not found", name);
                    1
                }
            }
            None => {
                eprintln!("lush: unalias: missing name");
                1
            }
        },
        "source" | "." => match args.get(0) {
            Some(path) => run_script_file(&expand_tilde(path), true),
            None => {
                eprintln!("lush: {}: missing filename", cmd);
                1
            }
        },
        "funcsave" => match args.get(0) {
            Some(name) => {
                let value = get_aliases().lock().unwrap().get(name).cloned();
                match value {
                    Some(value) => {
                        let dir = match env::var("HOME") {
                            Ok(home) => format!("{}/.config/lush/functions", home),
                            Err(_) => {
                                eprintln!("lush: funcsave: $HOME not set");
                                return 1;
                            }
                        };
                        if let Err(e) = std::fs::create_dir_all(&dir) {
                            eprintln!("lush: funcsave: {}: {}", dir, e);
                            return 1;
                        }
                        let path = format!("{}/{}.lush", dir, name);
                        let wraps = value.split_whitespace().next().unwrap_or(name);
                        let contents = format!(
                            "function {} --wraps={} --description 'alias {}={}'\n    {} $argv\nend\n",
                            name, wraps, name, value, value
                        );
                        match std::fs::write(&path, contents) {
                            Ok(()) => {
                                println!("Saved '{}' to {}", name, path);
                                0
                            }
                            Err(e) => {
                                eprintln!("lush: funcsave: {}: {}", path, e);
                                1
                            }
                        }
                    }
                    None => {
                        eprintln!("lush: funcsave: {}: no such alias defined", name);
                        1
                    }
                }
            }
            None => {
                eprintln!("lush: funcsave: missing name");
                1
            }
        },
        _ => unreachable!("run_builtin called with non-builtin: {}", cmd),
    }
}

fn exec_pipeline(cmds: &[SimpleCommand]) -> i32 {
    let mut children: Vec<std::process::Child> = Vec::new();
    let mut prev_stdout: Option<Stdio> = None;
    let mut last_builtin_status: Option<i32> = None;

    for (i, stage) in cmds.iter().enumerate() {
        let words = expand_alias(stage.words.clone());
        if words.is_empty() {
            eprintln!("lush: syntax error: missing command in pipeline");
            reap(children);
            return 1;
        }
        let cmd = words[0].as_str();
        let args = &words[1..];
        let is_last = i == cmds.len() - 1;
        let redirects = redirects_from_ast(&stage.redirects);

        if is_builtin(cmd) {
            let status = match cmd {
                "exit" => {
                    reap(children);
                    std::process::exit(0);
                }
                _ => run_builtin(cmd, args),
            };
            last_builtin_status = Some(status);
            prev_stdout = None;
            continue;
        }
        last_builtin_status = None;
        let stdin = if redirects.stdin.is_some() {
            match open_redirect_stdin(&redirects) {
                Some(s) => s,
                None => {
                    reap(children);
                    return 1;
                }
            }
        } else {
            prev_stdout.take().unwrap_or_else(Stdio::inherit)
        };

        let stdout = if redirects.stdout.is_some() {
            match open_redirect_stdout(&redirects) {
                Some(s) => s,
                None => {
                    reap(children);
                    return 1;
                }
            }
        } else if is_last {
            Stdio::inherit()
        } else {
            Stdio::piped()
        };

        let mut child = match Command::new(cmd)
            .args(args)
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("lush: {}: {}", cmd, e);
                reap(children);
                return 127;
            }
        };

        prev_stdout = child.stdout.take().map(Stdio::from);
        children.push(child);
    }

    let last_child_status = reap_and_get_last(children);
    last_builtin_status.unwrap_or(last_child_status)
}

fn reap(children: Vec<std::process::Child>) {
    for mut child in children {
        let _ = child.wait();
    }
}

fn reap_and_get_last(children: Vec<std::process::Child>) -> i32 {
    let mut last_code = 0;
    for mut child in children {
        last_code = match child.wait() {
            Ok(status) => status.code().unwrap_or(1),
            Err(_) => 1,
        };
    }
    last_code
}
