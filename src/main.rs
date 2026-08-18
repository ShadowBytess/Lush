<<<<<<< HEAD
use lush::{parse, Node, Redirect as AstRedirect, SimpleCommand};
=======
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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

<<<<<<< HEAD
=======
/// Bundles the pieces rustyline needs for tab-completion (path/filename
/// completion via the built-in FilenameCompleter) and fish-style
/// autosuggestions (ghost text pulled from history via HistoryHinter).
/// This mirrors the pattern from rustyline's own example.rs: derive
/// Completer/Hinter/Helper by forwarding to tagged fields, and implement
/// Highlighter/Validator by hand since there's nothing to derive them from
/// here (no bracket matching, no multi-line validation needed).
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
#[derive(Helper, Completer, Hinter)]
struct LushHelper {
    #[rustyline(Completer)]
    completer: FilenameCompleter,
    #[rustyline(Hinter)]
    hinter: HistoryHinter,
}

<<<<<<< HEAD
impl Validator for LushHelper {}

impl Highlighter for LushHelper {
=======
/// No custom validation rules (no multi-line input, no bracket matching),
/// so this just uses the trait's default "always valid" behavior.
impl Validator for LushHelper {}

impl Highlighter for LushHelper {
    /// Dims the inline autosuggestion text, approximating fish's grey
    /// ghost-text look with a plain ANSI "dim" escape rather than a real
    /// color (keeps this working over plain terminals without needing to
    /// know the user's palette).
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Owned(format!("\x1b[2m{}\x1b[0m", hint))
    }
}

<<<<<<< HEAD
=======
/// Connects two segments of a command chain, controlling whether the next
/// segment runs based on how the previous one exited.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ChainOp {
    And,  // && : run next only if previous succeeded (exit 0)
    Or,   // || : run next only if previous failed
    Then, // ;  : always run next, regardless of previous status
}

>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
fn main() {
    load_rc();

    let mut rl = Editor::<LushHelper, DefaultHistory>::new()
<<<<<<< HEAD
        .expect("lush: failed to initialize line editor");
    rl.set_helper(Some(LushHelper {
        completer: FilenameCompleter::new(),
        hinter: HistoryHinter::new(),
=======
    .expect("lush: failed to initialize line editor");
    rl.set_helper(Some(LushHelper {
        completer: FilenameCompleter::new(),
                       hinter: HistoryHinter::new(),
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
    }));

    let history_path = history_file_path();
    if let Some(ref path) = history_path {
<<<<<<< HEAD
        let _ = rl.load_history(path);
=======
        let _ = rl.load_history(path); // fine if it doesn't exist yet
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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
<<<<<<< HEAD
                continue;
            }
            Err(ReadlineError::Eof) => {
=======
                // Ctrl+C: just give a fresh prompt, matches bash/fish, not
                // an exit signal.
                continue;
            }
            Err(ReadlineError::Eof) => {
                // Ctrl+D
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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

<<<<<<< HEAD
=======
/// Where command history is persisted between sessions. Returns None (and
/// callers just skip load/save) if $HOME isn't set.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
fn history_file_path() -> Option<String> {
    env::var("HOME").ok().map(|home| format!("{}/.lush_history", home))
}

<<<<<<< HEAD
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

=======
/// Builds the prompt string handed to rustyline each time it reads a line
/// (replaces the old print!+flush approach, rustyline owns drawing the
/// prompt itself now).
fn build_prompt() -> String {
    let cwd = env::current_dir().unwrap_or_else(|_| ".".into());
    let dir_name = cwd
    .file_name()
    .map(|n| n.to_string_lossy().to_string())
    .unwrap_or_else(|| "/".to_string());

    format!("lush:{} $ ", dir_name)
}

/// Loads and runs `~/.lushrc` at startup, if it exists. This is where a
/// user would put `alias` definitions or any other startup commands
/// (they're run through the same execute_chain as anything typed
/// interactively, so `&&`, pipes, redirection all work in the rc file too).
/// Silently does nothing if the file isn't there, an rc file is optional.
///
/// Function-file aliases (see load_function_files) are loaded first, so an
/// `alias` line in .lushrc with the same name will override a same-named
/// `.lush` file, letting .lushrc act as the final word if you want it to.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
fn load_rc() {
    load_function_files();
    if let Ok(home) = env::var("HOME") {
        let path = format!("{}/.lushrc", home);
        run_script_file(&path, false);
    }
}

<<<<<<< HEAD
=======
/// Fish-style function autoloading: every `<name>.lush` file under
/// `~/.config/lush/functions/` becomes an alias called `<name>`, so instead
/// of a growing pile of `alias` lines in .lushrc, each one can live in its
/// own file. Unlike fish's *lazy* autoloading (which only reads a function
/// file the first time it's called), this reads every file up front at
/// startup, simpler, and fine for a handful of aliases.
///
/// Expected format, matching fish's function files:
///     function btctl --wraps=bluetoothctl --description 'alias btctl=bluetoothctl'
///         bluetoothctl $argv
///     end
/// `--wraps` is accepted but not used for anything (fish uses it for
/// completion inheritance, which lush doesn't have); `--description` is
/// parsed but currently only round-tripped through `funcsave`, not
/// displayed anywhere yet. `$argv` in the body is where the caller's
/// arguments get spliced in (see expand_alias); if you leave it out, args
/// just get appended at the end instead.
///
/// Files without a `function ... end` block fall back to the older plain
/// format (every non-comment line joined with spaces), so hand-written
/// files from before this format still work.
///
/// Silently does nothing if the directory doesn't exist yet, it's optional.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
fn load_function_files() {
    let dir = match env::var("HOME") {
        Ok(home) => format!("{}/.config/lush/functions", home),
        Err(_) => return,
    };

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
<<<<<<< HEAD
        Err(_) => return,
=======
        Err(_) => return, // no functions dir yet, nothing to load
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("lush") {
<<<<<<< HEAD
            continue;
=======
            continue; // ignore anything that isn't a .lush file
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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
<<<<<<< HEAD
=======
                // fish requires the function name to match its filename for
                // autoloading to find it; lush doesn't require it (it always
                // reads every file anyway) but a mismatch is almost always a
                // typo, so flag it.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
                if name != filename_stem {
                    eprintln!(
                        "lush: functions: warning: {} defines '{}', but filename suggests '{}'",
                        path.display(),
<<<<<<< HEAD
                        name,
                        filename_stem
=======
                              name,
                              filename_stem
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
                    );
                }
                (name, value)
            }
            None => {
<<<<<<< HEAD
                let value: String = contents
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .collect::<Vec<_>>()
                    .join(" ");
=======
                // Not a function-block file, fall back to the legacy plain
                // format: every non-blank, non-comment line joined with
                // spaces, aliased to the filename.
                let value: String = contents
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect::<Vec<_>>()
                .join(" ");
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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

<<<<<<< HEAD
=======
/// Parses a fish-style `function <name> [flags...]` / body / `end` block.
/// Returns None if the file doesn't start with a `function` header (after
/// skipping leading blank lines and `#` comments), signaling the caller to
/// fall back to the legacy plain-line format instead.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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

<<<<<<< HEAD
    let header_tokens = tokenize(header);
    let name = header_tokens.get(1)?.clone();
=======
    // Reuses the same quote-aware tokenizer as commands, so
    // --description 'text with spaces' comes back as one token.
    let header_tokens = tokenize(header);
    let name = header_tokens.get(1)?.clone();
    // header_tokens[2..] holds --wraps=... / --description '...' etc.
    // Currently unused beyond parsing, --wraps has no completion system
    // to feed here, and --description isn't surfaced anywhere yet.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7

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

<<<<<<< HEAD
=======
/// Reads a script file line by line and runs each non-blank, non-comment
/// (`#`) line through execute_chain. Used for both `~/.lushrc` at startup
/// and the `source`/`.` builtin. Returns 0 if the file was read
/// successfully, 1 if it couldn't be opened.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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

<<<<<<< HEAD
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

=======

/// Splits raw input on `&&`, `||`, and `;` (respecting double quotes) into
/// a sequence of command segments, each paired with the operator that
/// follows it (None for the final segment). Single `|` is deliberately left
/// alone here, pipe-splitting happens later per segment in `execute_line`.
fn split_chain(input: &str) -> Vec<(String, Option<ChainOp>)> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                in_quotes = !in_quotes;
                current.push(c);
                i += 1;
            }
            '&' if !in_quotes && chars.get(i + 1) == Some(&'&') => {
                segments.push((std::mem::take(&mut current), Some(ChainOp::And)));
                i += 2;
            }
            '|' if !in_quotes && chars.get(i + 1) == Some(&'|') => {
                segments.push((std::mem::take(&mut current), Some(ChainOp::Or)));
                i += 2;
            }
            ';' if !in_quotes => {
                segments.push((std::mem::take(&mut current), Some(ChainOp::Then)));
                i += 1;
            }
            _ => {
                current.push(c);
                i += 1;
            }
        }
    }
    segments.push((current, None));

    segments
}

/// Runs a full `cmd1 && cmd2 || cmd3 ; cmd4`-style chain, short-circuiting
/// each segment based on the previous segment's exit status.
fn execute_chain(input: &str) {
    let segments = split_chain(input);
    let mut should_run = true;
    let mut last_status = 0;

    for (text, op_after) in segments {
        let text = text.trim();
        if !text.is_empty() {
            if should_run {
                last_status = execute_line(text);
            }
            // else: skipped by short-circuit, last_status carries over
            // unchanged from whatever last actually ran.
        }

        should_run = match op_after {
            Some(ChainOp::Then) => true,
            Some(ChainOp::And) => should_run && last_status == 0,
            Some(ChainOp::Or) => !(should_run && last_status == 0),
            None => should_run, // chain is over, value unused
        };
    }
}

/// Splits a single chain segment on `|` and dispatches to run_single or
/// run_pipeline, returning the resulting exit code.
fn execute_line(line: &str) -> i32 {
    let commands: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
    if commands.len() == 1 {
        run_single(commands[0])
    } else {
        run_pipeline(&commands)
    }
}

/// Global alias table (`alias ll=ls -la` style). A Mutex is enough since
/// lush is single-threaded, this just needs interior mutability for a
/// value that outlives any single function call.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
fn get_aliases() -> &'static Mutex<HashMap<String, String>> {
    static ALIASES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    ALIASES.get_or_init(|| Mutex::new(HashMap::new()))
}

<<<<<<< HEAD
=======
/// If the first token matches a defined alias, replaces it with the
/// alias's expansion (re-tokenized). If the expansion contains a literal
/// `$argv` token (fish-style), the caller's remaining args are spliced in
/// at that exact position, matching the `bluetoothctl $argv` convention
/// from fish function files. If there's no `$argv` in the expansion, the
/// args are just appended at the end instead, preserving the simpler
/// `alias ll="ls -la"` behavior where there's nowhere else for them to go.
/// Deliberately only expands one level, no recursive alias-of-alias
/// resolution, so `alias ll=ll` can't create an infinite loop.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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

<<<<<<< HEAD
=======
/// Expands a leading `~` or `~/...` to $HOME. Falls back to `/` if $HOME
/// isn't set, which shouldn't happen in practice but keeps this infallible.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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

<<<<<<< HEAD
=======
/// Whether a bare token looks unambiguously like a directory reference
/// rather than a command name, gating fish-style implicit `cd`. Restricted
/// to `..`, `.`, `~`-prefixed, and anything containing `/`, deliberately
/// NOT bare single words like `Documents` even if such a directory exists,
/// since that's genuinely ambiguous with a real command of the same name
/// (fish only falls back to cd there after command lookup already failed;
/// replicating that ordering reliably would mean matching on the spawn
/// error's kind, which varies by platform/Rust version, so it's left out
/// here rather than done unreliably).
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
fn looks_like_path_token(token: &str) -> bool {
    token == ".." || token == "." || token.starts_with('~') || token.contains('/')
}

<<<<<<< HEAD
=======
/// Parses a command string into tokens, respecting quote grouping for both
/// double and single quotes (mixing styles within one input isn't
/// supported, whichever quote char opens a group is the only one that
/// closes it). Note: like a real shell, an unmatched quote will swallow
/// the rest of the line as part of one token rather than erroring, so a
/// stray apostrophe (e.g. `echo don't`) needs escaping or the other quote
/// style, same as bash.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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

<<<<<<< HEAD
=======
/// File-based redirection targets for a single command stage.
/// `stdin` overrides whatever would normally feed the command (inherited
/// terminal input, or the previous stage in a pipeline). `stdout` overrides
/// whatever would normally receive output (terminal, or the next stage).
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
#[derive(Default)]
struct Redirects {
    stdin: Option<String>,
    stdout: Option<String>,
    append: bool, // true for >>, false for >
}

<<<<<<< HEAD
=======
/// Strips `<file`, `>file`, and `>>file` out of a token list, returning the
/// remaining command+args plus whatever redirects were found. Operators and
/// their filename must currently be separate tokens (`> out.txt`, not
/// `>out.txt` glued together), matching how the existing tokenizer splits
/// on whitespace.
fn parse_redirects(tokens: Vec<String>) -> (Vec<String>, Redirects) {
    let mut clean = Vec::new();
    let mut redirects = Redirects::default();
    let mut iter = tokens.into_iter().peekable();

    while let Some(tok) = iter.next() {
        match tok.as_str() {
            "<" => {
                if let Some(file) = iter.next() {
                    redirects.stdin = Some(file);
                } else {
                    eprintln!("lush: syntax error: expected filename after '<'");
                }
            }
            ">" => {
                if let Some(file) = iter.next() {
                    redirects.stdout = Some(file);
                    redirects.append = false;
                } else {
                    eprintln!("lush: syntax error: expected filename after '>'");
                }
            }
            ">>" => {
                if let Some(file) = iter.next() {
                    redirects.stdout = Some(file);
                    redirects.append = true;
                } else {
                    eprintln!("lush: syntax error: expected filename after '>>'");
                }
            }
            _ => clean.push(tok),
        }
    }

    (clean, redirects)
}

/// Opens the files a Redirects struct points to and returns Stdio handles
/// ready to hand to Command. Returns None if a file couldn't be opened,
/// after printing an error, so the caller can bail without spawning.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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
<<<<<<< HEAD
                .write(true)
                .create(true)
                .append(redirects.append)
                .truncate(!redirects.append)
                .open(path);
=======
            .write(true)
            .create(true)
            .append(redirects.append)
            .truncate(!redirects.append)
            .open(path);
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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

<<<<<<< HEAD
=======
fn run_single(line: &str) -> i32 {
    let tokens = tokenize(line);
    if tokens.is_empty() {
        return 0;
    }
    let tokens = expand_alias(tokens);

    let (tokens, redirects) = parse_redirects(tokens);
    if tokens.is_empty() {
        // e.g. someone typed only "> out.txt" with no command
        eprintln!("lush: syntax error: missing command");
        return 1;
    }

    let cmd = tokens[0].as_str();
    let args = &tokens[1..];

    // Fish-style implicit cd: a bare directory reference with no other
    // args (`..`, `~/Projects`, `subdir/child`) changes into it directly,
    // no `cd` needed. Checked up front rather than as a spawn-failure
    // fallback, so it doesn't depend on how the OS reports "tried to exec
    // a directory" (that error kind isn't consistent to match on).
    if args.is_empty() && looks_like_path_token(cmd) {
        let expanded = expand_tilde(cmd);
        if Path::new(&expanded).is_dir() {
            return run_builtin("cd", &[cmd.to_string()]);
        }
    }

    // Builtins have to run in-process (can't fork for cd/exit/alias/source),
    // and don't meaningfully support redirection, so it's silently ignored
    // for them.
    match cmd {
        "exit" => std::process::exit(0),
        _ if is_builtin(cmd) => run_builtin(cmd, args),
        _ => {
            let stdin = match open_redirect_stdin(&redirects) {
                Some(s) => s,
                None => return 1, // error already printed
            };
            let stdout = match open_redirect_stdout(&redirects) {
                Some(s) => s,
                None => return 1,
            };

            match Command::new(cmd).args(args).stdin(stdin).stdout(stdout).spawn() {
                Ok(mut child) => match child.wait() {
                    Ok(status) => status.code().unwrap_or(1),
                    Err(_) => 1,
                },
                Err(e) => {
                    eprintln!("lush: {}: {}", cmd, e);
                    127 // conventional "command not found" exit code
                }
            }
        }
    }
}

/// Commands that must run in-process rather than being spawned.
/// Note: cd/exit inside a pipeline still only affect the shell itself if
/// they're the *only* stage doing the work; a true subshell-per-stage
/// semantics is out of scope for now, this just stops them from being
/// treated as (nonexistent) external binaries.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
fn is_builtin(cmd: &str) -> bool {
    matches!(cmd, "cd" | "exit" | "alias" | "unalias" | "source" | "." | "funcsave")
}

<<<<<<< HEAD
=======
/// Runs any builtin except `exit`, which every caller handles separately
/// since it needs to reap children / exit the whole process rather than
/// just return a status code.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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
<<<<<<< HEAD
=======
                // No args: list all current aliases, alphabetically.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
                let aliases = get_aliases().lock().unwrap();
                let mut names: Vec<&String> = aliases.keys().collect();
                names.sort();
                for name in names {
                    println!("alias {}='{}'", name, aliases[name]);
                }
                return 0;
            }
<<<<<<< HEAD

=======
            // Tokens got split on spaces, so `alias ll="ls -la"` arrives as
            // one token ("ll=ls -la", quotes already stripped by tokenize)
            // in the common case, but join defensively in case someone
            // wrote spaces around the '='.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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
<<<<<<< HEAD
=======
                        // Write in the same function/end format fish uses.
                        // --wraps guesses at the underlying command (first
                        // word of the expansion); $argv is added explicitly
                        // in the body so args pass through on invocation.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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

<<<<<<< HEAD
fn exec_pipeline(cmds: &[SimpleCommand]) -> i32 {
    let mut children: Vec<std::process::Child> = Vec::new();
    let mut prev_stdout: Option<Stdio> = None;
    let mut last_builtin_status: Option<i32> = None;

    for (i, stage) in cmds.iter().enumerate() {
        let words = expand_alias(stage.words.clone());
        if words.is_empty() {
=======
/// Runs a pipeline of commands like `cmd1 | cmd2 | cmd3`, returning the
/// exit code of the last stage (matching typical shell behavior without
/// pipefail).
fn run_pipeline(commands: &[&str]) -> i32 {
    let mut children: Vec<std::process::Child> = Vec::new();
    let mut prev_stdout: Option<Stdio> = None;
    // Tracks the exit code when the last stage turns out to be a builtin
    // (which never becomes a Child, so it can't be read off `children`).
    let mut last_builtin_status: Option<i32> = None;

    for (i, cmd_str) in commands.iter().enumerate() {
        let tokens = tokenize(cmd_str);
        if tokens.is_empty() {
            // Empty stage (e.g. "cmd1 || cmd2" mistakenly split, or trailing
            // pipe). Bail out cleanly rather than spawning nothing and
            // hanging the next stage's stdin.
            eprintln!("lush: syntax error: empty command in pipeline");
            reap(children);
            return 1;
        }
        let tokens = expand_alias(tokens);
        let (tokens, redirects) = parse_redirects(tokens);
        if tokens.is_empty() {
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
            eprintln!("lush: syntax error: missing command in pipeline");
            reap(children);
            return 1;
        }
<<<<<<< HEAD
        let cmd = words[0].as_str();
        let args = &words[1..];
        let is_last = i == cmds.len() - 1;
        let redirects = redirects_from_ast(&stage.redirects);

        if is_builtin(cmd) {
=======
        let cmd = tokens[0].as_str();
        let args = &tokens[1..];
        let is_last = i == commands.len() - 1;

        if is_builtin(cmd) {
            // Builtins can't be spawned as a child process, so they can't
            // participate in the pipe chain the normal way. Run them
            // in-process; if they're not the last stage there's no
            // meaningful output to hand downstream, so just skip forward.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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
<<<<<<< HEAD
        last_builtin_status = None;
=======
        last_builtin_status = None; // this stage is external, overrides any earlier builtin

        // An explicit `< file` on this stage wins over whatever the previous
        // stage would have piped in (matches typical shell behavior, this
        // stage just ignores upstream data and reads the file instead).
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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

<<<<<<< HEAD
=======
        // An explicit `> file` / `>> file` on this stage wins over piping to
        // the next stage. This mirrors real shells: `cmd1 | cmd2 > out.txt`
        // sends cmd2's output to the file, not to a (nonexistent) cmd3.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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
<<<<<<< HEAD
            .args(args)
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::inherit())
            .spawn()
=======
        .args(args)
        .stdin(stdin)
        .stdout(stdout)
        .stderr(Stdio::inherit()) // let each stage's errors surface directly
        .spawn()
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("lush: {}: {}", cmd, e);
<<<<<<< HEAD
=======
                // Don't leave earlier stages running with nowhere for their
                // output to go, wait on what's already spawned before bailing.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
                reap(children);
                return 127;
            }
        };

        prev_stdout = child.stdout.take().map(Stdio::from);
        children.push(child);
    }

    let last_child_status = reap_and_get_last(children);
<<<<<<< HEAD
    last_builtin_status.unwrap_or(last_child_status)
}

=======
    // If the final stage was a builtin, its status takes precedence, it
    // never went through `children` in the first place.
    last_builtin_status.unwrap_or(last_child_status)
}

/// Waits on every child in the pipeline so none are left as zombies,
/// even if an earlier stage in the chain failed. Discards exit codes,
/// used on error paths where a fixed status is returned instead.
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
fn reap(children: Vec<std::process::Child>) {
    for mut child in children {
        let _ = child.wait();
    }
}

<<<<<<< HEAD
=======
/// Waits on every child, same as `reap`, but returns the exit code of the
/// last one in the list (i.e. the last external stage in the pipeline).
/// Returns 0 if the list is empty (e.g. a pipeline that was entirely
/// builtins, though that's an unusual edge case).
>>>>>>> 17428d44d607c39929e2223596223cb45c8d41b7
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
