use lush::{expand_word, glob_expand_word, lex, parse, parse_assignment, Node, Redirect as AstRedirect, SimpleCommand, Token, Word, WordPart};
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
use std::ffi::c_int;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::io::FromRawFd;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

/// Bundles the pieces rustyline needs for tab-completion (path/filename
/// completion via the built-in FilenameCompleter) and fish-style
/// autosuggestions (ghost text pulled from history via HistoryHinter).
/// This mirrors the pattern from rustyline's own example.rs: derive
/// Completer/Hinter/Helper by forwarding to tagged fields, and implement
/// Highlighter/Validator by hand since there's nothing to derive them from
/// here (no bracket matching, no multi-line validation needed).
#[derive(Helper, Completer, Hinter)]
struct LushHelper {
    #[rustyline(Completer)]
    completer: FilenameCompleter,
    #[rustyline(Hinter)]
    hinter: HistoryHinter,
}

/// No custom validation rules (no multi-line input, no bracket matching),
/// so this just uses the trait's default "always valid" behavior.
impl Validator for LushHelper {}

impl Highlighter for LushHelper {
    /// Dims the inline autosuggestion text, approximating fish's grey
    /// ghost-text look with a plain ANSI "dim" escape rather than a real
    /// color (keeps this working over plain terminals without needing to
    /// know the user's palette).
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Owned(format!("\x1b[2m{}\x1b[0m", hint))
    }
}

fn main() {
    // Initialize $PWD before anything else runs (including .lushrc), so
    // it's correct from the very first prompt rather than only becoming
    // accurate after the first `cd`.
    // SAFETY: this runs before any other code touches the environment,
    // nothing else in this single-threaded process could be racing it.
    if let Ok(cwd) = env::current_dir() {
        unsafe {
            env::set_var("PWD", cwd.as_os_str());
        }
    }

    load_rc();

    let mut rl = Editor::<LushHelper, DefaultHistory>::new()
        .expect("lush: failed to initialize line editor");
    rl.set_helper(Some(LushHelper {
        completer: FilenameCompleter::new(),
        hinter: HistoryHinter::new(),
    }));

    let history_path = history_file_path();
    if let Some(ref path) = history_path {
        let _ = rl.load_history(path); // fine if it doesn't exist yet
    }

    let mut exit_code = 0;
    loop {
        let prompt = build_prompt();
        match rl.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(line);
                if let Some(code) = execute_chain(line) {
                    exit_code = code;
                    break;
                }
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl+C: just give a fresh prompt, matches bash/fish, not
                // an exit signal.
                continue;
            }
            Err(ReadlineError::Eof) => {
                // Ctrl+D
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
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

/// Where command history is persisted between sessions. Returns None (and
/// callers just skip load/save) if $HOME isn't set.
fn history_file_path() -> Option<String> {
    env::var("HOME").ok().map(|home| format!("{}/.lush_history", home))
}

/// Builds the prompt string handed to rustyline each time it reads a line
/// (replaces the old print!+flush approach, rustyline owns drawing the
/// prompt itself now).
fn build_prompt() -> String {
    let cwd = env::current_dir().unwrap_or_else(|_| "/".into());
    let cwd_str = cwd.to_string_lossy().to_string();
    let home = env::var("HOME").unwrap_or_default();

    // Show the cwd with $HOME collapsed to ~, same convention as bash/fish.
    let display_path = if !home.is_empty() && cwd_str == home {
        "~".to_string()
    } else if !home.is_empty() && cwd_str.starts_with(&format!("{}/", home)) {
        format!("~{}", &cwd_str[home.len()..])
    } else {
        cwd_str
    };

    format!("{}: {}@{}\n⟩ ", display_path, get_username(), get_hostname())
}

/// Current user's login name. Falls back through a couple of env vars
/// before giving up with a generic placeholder, should basically never
/// actually hit that fallback on a normal Linux setup.
fn get_username() -> String {
    env::var("USER")
        .or_else(|_| env::var("LOGNAME"))
        .unwrap_or_else(|_| "user".to_string())
}

/// System hostname. Reads it straight from /proc rather than spawning a
/// process, since this runs on every single prompt draw and a subprocess
/// per keystroke-adjacent redraw would be wasteful. Falls back to the
/// `hostname` command (in case /proc isn't mounted, e.g. some containers),
/// then a generic placeholder as a last resort.
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

/// Loads and runs `~/.lushrc` at startup, if it exists. This is where a
/// user would put `alias` definitions or any other startup commands
/// (they're run through the same execute_chain as anything typed
/// interactively, so `&&`, pipes, redirection all work in the rc file too).
/// Silently does nothing if the file isn't there, an rc file is optional.
///
/// Function-file aliases (see load_function_files) are loaded first, so an
/// `alias` line in .lushrc with the same name will override a same-named
/// `.lush` file, letting .lushrc act as the final word if you want it to.
fn load_rc() {
    load_function_files();
    if let Ok(home) = env::var("HOME") {
        let path = format!("{}/.lushrc", home);
        run_script_file(&path, false);
    }
}

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
fn load_function_files() {
    let dir = match env::var("HOME") {
        Ok(home) => format!("{}/.config/lush/functions", home),
        Err(_) => return,
    };

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return, // no functions dir yet, nothing to load
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("lush") {
            continue; // ignore anything that isn't a .lush file
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
                // fish requires the function name to match its filename for
                // autoloading to find it; lush doesn't require it (it always
                // reads every file anyway) but a mismatch is almost always a
                // typo, so flag it.
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
                // Not a function-block file, fall back to the legacy plain
                // format: every non-blank, non-comment line joined with
                // spaces, aliased to the filename.
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

/// Parses a fish-style `function <name> [flags...]` / body / `end` block.
/// Returns None if the file doesn't start with a `function` header (after
/// skipping leading blank lines and `#` comments), signaling the caller to
/// fall back to the legacy plain-line format instead.
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

    // Reuses lib.rs's quote-aware lexer — the exact same one typed commands
    // go through, retired duplicate tokenizer and all — so --description
    // 'text with spaces' comes back as one word. Only Word tokens are kept;
    // a header can't meaningfully contain operators anyway.
    let header_tokens: Vec<String> = lex(header)
        .into_iter()
        .filter_map(|t| match t {
            Token::Word(w) => Some(w.text()),
            _ => None,
        })
        .collect();
    let name = header_tokens.get(1)?.clone();
    // header_tokens[2..] holds --wraps=... / --description '...' etc.
    // Currently unused beyond parsing, --wraps has no completion system
    // to feed here, and --description isn't surfaced anywhere yet.

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

/// Reads a script file line by line and runs each non-blank, non-comment
/// (`#`) line through execute_chain. Used for both `~/.lushrc` at startup
/// and the `source`/`.` builtin. Returns 0 if the file was read
/// successfully, 1 if it couldn't be opened. If a line requests a shell
/// exit (`exit` builtin), execution of the script stops there and the
/// exit code propagates — the interactive shell still shuts down cleanly
/// through its normal path, preserving history.
fn run_script_file(path: &str, warn_if_missing: bool) -> i32 {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some(code) = execute_chain(line) {
                    return code;
                }
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


/// Parses a full line into an AST (via lib.rs's lexer/parser) and walks
/// it. This is the executor swap: previously this function did its own
/// string-splitting on &&/||/; (split_chain) and then further split each
/// segment on `|` (execute_line) before tokenizing. All of that is now
/// one call to `parse()`, which handles quoting correctly across the
/// whole input in one pass, fixing the bugs the old approach had (a `|`
/// inside quotes no longer gets mistaken for a pipeline).
///
/// Returns `Some(exit_code)` when the chain requested a shell exit (via
/// the `exit` builtin), so the caller can shut down gracefully — saving
/// history, restoring terminal state — instead of vanishing mid-walk like
/// the old direct process::exit did (which silently discarded the whole
/// session's history). `None` means keep reading lines.
fn execute_chain(input: &str) -> Option<i32> {
    match parse(input) {
        Ok(Some(node)) => {
            let status = exec_node(&node, true);
            if status >= EXIT_SENTINEL {
                Some(status - EXIT_SENTINEL)
            } else {
                None
            }
        }
        Ok(None) => None, // empty input, nothing to run
        Err(e) => {
            eprintln!("lush: {}", e);
            None
        }
    }
}

/// Base value for the internal "exit requested" signal returned up through
/// the executor. Real command statuses are 0..=255, so 256+ can never
/// collide; the desired exit code rides along as SENTINEL + code.
const EXIT_SENTINEL: i32 = 256;

/// The `exit` builtin, expressed as an executor status rather than an
/// immediate process::exit — see execute_chain for why. With no argument,
/// exits 0 (lush doesn't track a `$?` yet); with a numeric argument,
/// exits with that value; anything else errors and exits 2, matching
/// bash's "numeric argument required".
fn exit_requested(args: &[String]) -> i32 {
    match args.first().map(|s| s.as_str()) {
        None => EXIT_SENTINEL,
        Some(arg) => match arg.parse::<u8>() {
            Ok(code) => EXIT_SENTINEL + code as i32,
            Err(_) => {
                eprintln!("lush: exit: {}: numeric argument required", arg);
                EXIT_SENTINEL + 2
            }
        },
    }
}

/// Walks an AST node, dispatching to the right executor and propagating
/// exit status exactly like the old flattened should_run/last_status
/// bookkeeping did, the tree's left-associative shape already encodes the
/// same short-circuit order, so no separate state tracking is needed here.
///
/// `expand_aliases` controls whether the first word of each simple
/// command is looked up in the alias table. It's threaded through the
/// whole tree rather than being a one-shot flag because an alias's own
/// expansion is itself a full subtree that gets executed by these same
/// walkers — with expansion turned OFF. That's what keeps the documented
/// one-level-only rule intact (an alias can't invoke another alias), and
/// it's what makes `alias ll=ll` terminate instead of looping forever.
fn exec_node(node: &Node, expand_aliases: bool) -> i32 {
    match node {
        Node::Command(cmd) => exec_simple_command(cmd, expand_aliases),
        Node::Pipeline(cmds) => exec_pipeline(cmds, expand_aliases),
        Node::And(lhs, rhs) => {
            let status = exec_node(lhs, expand_aliases);
            if status >= EXIT_SENTINEL {
                return status;
            }
            if status == 0 {
                exec_node(rhs, expand_aliases)
            } else {
                status
            }
        }
        Node::Or(lhs, rhs) => {
            let status = exec_node(lhs, expand_aliases);
            if status >= EXIT_SENTINEL {
                return status;
            }
            if status != 0 {
                exec_node(rhs, expand_aliases)
            } else {
                status
            }
        }
        Node::Sequence(lhs, rhs) => {
            let status = exec_node(lhs, expand_aliases);
            if status >= EXIT_SENTINEL {
                return status;
            }
            exec_node(rhs, expand_aliases)
        }
    }
}

/// Folds an AST command's redirect list into the existing Redirects
/// struct (last-one-wins per kind, same as a real shell: `cmd > a > b`
/// ends up writing to `b`). Keeps open_redirect_stdin/open_redirect_stdout
/// unchanged, they don't need to know or care that redirects now come
/// from the parser instead of parse_redirects().
fn redirects_from_ast(redirects: &[AstRedirect]) -> Redirects {
    let mut r = Redirects::default();
    for redirect in redirects {
        match redirect {
            AstRedirect::In(word) => r.stdin = Some(expand_word(word, &lookup_variable)),
            AstRedirect::Out(word) => {
                r.stdout = Some(expand_word(word, &lookup_variable));
                r.append = false;
            }
            AstRedirect::Append(word) => {
                r.stdout = Some(expand_word(word, &lookup_variable));
                r.append = true;
            }
            AstRedirect::ErrOut(word) => {
                r.stderr = Some(expand_word(word, &lookup_variable));
                r.stderr_append = false;
                r.stderr_to_stdout = false;
            }
            AstRedirect::ErrAppend(word) => {
                r.stderr = Some(expand_word(word, &lookup_variable));
                r.stderr_append = true;
                r.stderr_to_stdout = false;
            }
            AstRedirect::ErrToStdout => {
                r.stderr_to_stdout = true;
                r.stderr = None; // duplicated onto stdout, not a file
            }
            AstRedirect::Both(word) => {
                let path = expand_word(word, &lookup_variable);
                r.stdout = Some(path.clone());
                r.append = false;
                r.stderr = Some(path);
                r.stderr_append = false;
                r.stderr_to_stdout = false;
            }
        }
    }
    r
}

/// Executes a single parsed command (replaces the old run_single, now
/// operating on a SimpleCommand from the AST instead of re-tokenizing a
/// string). Behavior is unchanged: alias expansion, implicit cd, builtin
/// dispatch, then external spawn with redirection.
fn exec_simple_command(cmd: &SimpleCommand, expand_aliases: bool) -> i32 {
    // Alias expansion is AST-level now (see build_alias_invocation): an
    // alias's value goes through the same lex/parse path as typed input,
    // and the resulting node — however many operators it contains —
    // replaces this command wholesale. It's executed with expand_aliases
    // turned off so the expansion is applied exactly once, preserving the
    // documented one-level rule.
    if expand_aliases {
        if let Some(first) = cmd.words.first() {
            let name = first.text();
            if let Some(value) = get_aliases().lock().unwrap().get(&name).cloned() {
                return exec_alias_invocation(&name, &value, cmd);
            }
        }
    }

    // Standalone `name=value` assignment: not a command to execute, sets
    // a shell-local variable instead. Only the single-word form is
    // recognized (`foo=bar echo hi`, bash's temporary per-command
    // environment form, is a separate feature, not implemented here).
    // Checked after the alias gate but before general word/variable
    // expansion, since the value's own quote-tracking needs to survive
    // intact into expand_word (so `foo='$HOME'` stays literal).
    if let Some((name, value_word)) = parse_assignment(&cmd.words) {
        let value = expand_word(&value_word, &lookup_variable);
        get_shell_vars().lock().unwrap().insert(name, value);
        return 0;
    }

    let words = expand_command_words(&cmd.words);
    if words.is_empty() {
        // e.g. a command that was only a redirect, "> out.txt" with
        // nothing to run, or an alias that expanded to nothing.
        eprintln!("lush: syntax error: missing command");
        return 1;
    }

    let cmd_name = words[0].as_str();
    let args = &words[1..];

    // Fish-style implicit cd: a bare directory reference with no other
    // args (`..`, `~/Projects`, `subdir/child`) changes into it directly,
    // no `cd` needed.
    if args.is_empty() && looks_like_path_token(cmd_name) {
        let expanded = expand_tilde(cmd_name);
        if Path::new(&expanded).is_dir() {
            let mut out = Box::new(io::stdout()) as Box<dyn Write>;
            let mut err = Box::new(io::stderr()) as Box<dyn Write>;
            return run_builtin("cd", &[cmd_name.to_string()], out.as_mut(), err.as_mut());
        }
    }

    let redirects = redirects_from_ast(&cmd.redirects);

    match cmd_name {
        "exit" => exit_requested(args),
        _ if is_builtin(cmd_name) => {
            match (open_builtin_stdout(&redirects), open_builtin_stderr(&redirects)) {
                (Some(mut out), Some(mut err)) => {
                    run_builtin(cmd_name, args, out.as_mut(), err.as_mut())
                }
                _ => 1, // a redirect target failed to open; error already printed
            }
        }
        _ => {
            let stdin = match open_redirect_stdin(&redirects) {
                Some(s) => s,
                None => return 1, // error already printed
            };
            let stdout = match open_redirect_stdout(&redirects) {
                Some(s) => s,
                None => return 1,
            };
            // Not in a pipeline here, stdout is never being piped to a
            // next stage, so stdout_is_piped is always false.
            let stderr = match open_redirect_stderr(&redirects) {
                Some(s) => s,
                None => return 1,
            };

            match Command::new(cmd_name)
                .args(args)
                .stdin(stdin)
                .stdout(stdout)
                .stderr(stderr)
                .spawn()
            {
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

/// Global alias table (`alias ll=ls -la` style). A Mutex is enough since
/// lush is single-threaded, this just needs interior mutability for a
/// value that outlives any single function call.
fn get_aliases() -> &'static Mutex<HashMap<String, String>> {
    static ALIASES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    ALIASES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Global table of shell-local variables (`name=value` assignments).
/// Being "exported" (see the `export` builtin) just means the same value
/// is *also* mirrored into the real process environment via
/// `env::set_var`, so child processes inherit it, this table is always
/// checked first during `$VAR` lookup regardless, so a local assignment
/// can shadow an inherited environment variable of the same name.
fn get_shell_vars() -> &'static Mutex<HashMap<String, String>> {
    static SHELL_VARS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    SHELL_VARS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Executes `name`, known to be an alias bound to `value`, in place of
/// the invoking command `invoking` (whose words[1..] are the caller's
/// arguments and whose redirects were written at the invocation site).
///
/// This is the final piece of the executor swap. Previously expand_alias()
/// re-lexed an alias's value but kept only its Word tokens, silently
/// discarding every operator — `alias gs='git status && git diff'` would
/// actually run `git status git diff`. Aliases are now parsed through the
/// same lex()/parse() path as typed input, so `&&`, `||`, `;`, pipelines,
/// and redirects inside an alias's value all behave exactly as if they'd
/// been typed, and the expansion runs as a full subtree (with alias
/// expansion off, keeping the one-level rule).
fn exec_alias_invocation(name: &str, value: &str, invoking: &SimpleCommand) -> i32 {
    match build_alias_invocation(name, value, &invoking.words[1..], &invoking.redirects) {
        Ok(Some(node)) => exec_node(&node, false),
        // An empty alias value does nothing; same spirit as bash running
        // an empty command (status 0), not a syntax error.
        Ok(None) => 0,
        Err(msg) => {
            eprintln!("lush: {}", msg);
            1
        }
    }
}

/// Builds the AST node an alias invocation should execute, without any
/// side effects: parses the alias's `value` through the same lex/parse
/// path as typed input, inserts the caller's arguments ($argv splice or
/// append-to-last), and attaches the invocation-site redirects to the
/// last command.
///
/// Returns Ok(None) when the value is empty (nothing to run), Err with a
/// ready-to-print message when the value doesn't parse.
///
/// This is the final piece of the executor swap. Previously expand_alias()
/// re-lexed an alias's value but kept only its Word tokens, silently
/// discarding every operator — `alias gs='git status && git diff'` would
/// actually run `git status git diff`. Aliases now behave exactly as if
/// their value had been typed: `&&`, `||`, `;`, pipelines, and redirects
/// all work, and the expansion runs as a full subtree with alias
/// expansion off (the one-level rule).
fn build_alias_invocation(
    name: &str,
    value: &str,
    args: &[Word],
    site_redirects: &[AstRedirect],
) -> Result<Option<Node>, String> {
    let node = match parse(value) {
        Ok(Some(node)) => node,
        Ok(None) => return Ok(None),
        Err(e) => return Err(format!("alias '{}': {}", name, e)),
    };
    let mut node = insert_args_into_node(node, args);
    attach_redirects_to_last_command(&mut node, site_redirects);
    Ok(Some(node))
}

/// Inserts caller argument words into a single command: splices them in
/// place of its FIRST `$argv` word, or — when there is no `$argv` —
/// appends them at the end. The append target mirrors bash's textual
/// substitution, where trailing arguments land after the end of the
/// substituted text (`two='echo one && echo tail'` + args →
/// `echo one && echo tail extra`); the `$argv` splice matches the
/// fish-style convention lush function files already rely on.
fn insert_args_into_command(cmd: &mut SimpleCommand, args: &[Word]) {
    if !try_splice_argv_into_command(cmd, args) {
        cmd.words.extend_from_slice(args);
    }
}

/// Splices `args` into `cmd`'s words in place of its first `$argv` word.
/// Returns whether a `$argv` was found; when false, the words are left
/// untouched (callers use that to fall back to appending).
fn try_splice_argv_into_command(cmd: &mut SimpleCommand, args: &[Word]) -> bool {
    match cmd.words.iter().position(|w| w.text() == "$argv") {
        Some(pos) => {
            cmd.words.splice(pos..pos + 1, args.iter().cloned());
            true
        }
        None => false,
    }
}

/// Node-level form of try_splice_argv_into_command: searches the tree in
/// evaluation order (left to right, depth first) for the first word that
/// is exactly `$argv`, replaces it with `args`, and reports success.
fn try_splice_argv(node: &mut Node, args: &[Word]) -> bool {
    match node {
        Node::Command(cmd) => try_splice_argv_into_command(cmd, args),
        // Pipeline stages are plain commands, not nested Nodes.
        Node::Pipeline(stages) => stages
            .iter_mut()
            .any(|stage| try_splice_argv_into_command(stage, args)),
        Node::And(lhs, rhs) | Node::Or(lhs, rhs) | Node::Sequence(lhs, rhs) => {
            try_splice_argv(lhs, args) || try_splice_argv(rhs, args)
        }
    }
}

/// Runs `edit` on the LAST command in the tree's evaluation order (the
/// rightmost leaf) — the place textual substitution would have deposited
/// anything that followed the aliased name.
fn edit_last_command(node: &mut Node, edit: &mut dyn FnMut(&mut SimpleCommand)) {
    match node {
        Node::Command(cmd) => edit(cmd),
        Node::Pipeline(stages) => {
            if let Some(last) = stages.last_mut() {
                edit(last);
            }
        }
        Node::And(_, rhs) | Node::Or(_, rhs) | Node::Sequence(_, rhs) => {
            edit_last_command(rhs, edit)
        }
    }
}

/// Inserts `args` into an expanded alias node per the $argv-or-append rule
/// described on insert_args_into_command.
fn insert_args_into_node(mut node: Node, args: &[Word]) -> Node {
    if !try_splice_argv(&mut node, args) {
        edit_last_command(&mut node, &mut |cmd| cmd.words.extend_from_slice(args));
    }
    node
}

/// Attaches redirects written at the invocation site (`gs > out.txt`)
/// onto the expansion's LAST command, again mirroring how textual
/// substitution would order them after everything else. Redirect folding
/// later (redirects_from_ast) is last-one-wins per kind, so a site-level
/// redirect also overrides any same-kind redirect the alias's own value
/// carried — the same precedence typed input would produce.
fn attach_redirects_to_last_command(node: &mut Node, redirects: &[AstRedirect]) {
    if redirects.is_empty() {
        return;
    }
    let owned = redirects.to_vec();
    edit_last_command(node, &mut move |cmd| {
        cmd.redirects.extend(owned.iter().cloned());
    });
}

/// Looks up a variable's value for `$VAR`/`${VAR}` expansion. Checks
/// shell-local variables first (so `foo=bar` can shadow an inherited
/// environment variable named `foo`), then falls back to the real
/// process environment (`$HOME`, `$USER`, `$PATH`, etc., inherited at
/// startup, never explicitly set within Lush). Returning `None` here is
/// what makes an undefined variable expand to an empty string rather
/// than error, matching real shells.
fn lookup_variable(name: &str) -> Option<String> {
    if let Some(value) = get_shell_vars().lock().unwrap().get(name) {
        return Some(value.clone());
    }
    env::var(name).ok()
}

/// Builds a command's final argv: per word, variable expansion followed
/// by filename globbing of the result (so `$HOME/*.txt` works).
///
/// Two bash-parity rules apply to the result:
/// - A word whose UNQUOTED expansion is empty vanishes from argv entirely
///   (`echo A $UNSET B` prints "A B", and `ls $UNSET` lists the directory
///   instead of erroring on a phantom "" argument).
/// - A word that was explicitly quoted keeps its (possibly empty)
///   argument: `echo a "" b` still passes an empty string, which is why
///   the lexer emits zero-length parts for quoted empties.
///
/// A word whose glob pattern matches nothing stays literal — bash's
/// default nullglob-off behavior, so `rm *.txt` with no .txt files fails
/// exactly the way bash's rm would instead of silently vanishing.
///
/// Redirect targets are deliberately NOT globbed here; real shells reject
/// ambiguous redirect targets outright (`> *.txt` is an error in bash),
/// which is its own feature and not something lush does yet.
fn expand_command_words(words: &[Word]) -> Vec<String> {
    let mut out = Vec::with_capacity(words.len());
    for w in words {
        let matches = glob_expand_word(w, &lookup_variable);
        if !matches.is_empty() {
            out.extend(matches);
            continue;
        }
        let text = expand_word(w, &lookup_variable);
        // Drop only words that are empty AND had nothing quoted in them:
        // any Literal/DoubleQuoted part means the user wrote quotes here,
        // and a quoted empty is a real (empty) argument.
        if text.is_empty() && w.parts.iter().all(|p| matches!(p, WordPart::Expandable(_))) {
            continue;
        }
        out.push(text);
    }
    out
}

/// Expands a leading `~` or `~/...` to $HOME. Falls back to `/` if $HOME
/// isn't set, which shouldn't happen in practice but keeps this infallible.
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

/// Whether a bare token looks unambiguously like a directory reference
/// rather than a command name, gating fish-style implicit `cd`. Restricted
/// to `..`, `.`, `~`-prefixed, and anything containing `/`, deliberately
/// NOT bare single words like `Documents` even if such a directory exists,
/// since that's genuinely ambiguous with a real command of the same name
/// (fish only falls back to cd there after command lookup already failed;
/// replicating that ordering reliably would mean matching on the spawn
/// error's kind, which varies by platform/Rust version, so it's left out
/// here rather than done unreliably).
fn looks_like_path_token(token: &str) -> bool {
    token == ".." || token == "." || token.starts_with('~') || token.contains('/')
}

/// File-based redirection targets for a single command stage.
/// `stdin` overrides whatever would normally feed the command (inherited
/// terminal input, or the previous stage in a pipeline). `stdout` overrides
/// whatever would normally receive output (terminal, or the next stage).
/// `stderr`/`stderr_append` mirror `stdout`/`append` but for `2>`/`2>>`.
/// `stderr_to_stdout` is `2>&1`: stderr duplicated onto wherever stdout
/// is currently headed, rather than a separate file, see
/// open_redirect_stderr for how that's actually resolved.
#[derive(Default)]
struct Redirects {
    stdin: Option<String>,
    stdout: Option<String>,
    append: bool, // true for >>, false for >
    stderr: Option<String>,
    stderr_append: bool, // true for 2>>, false for 2>
    stderr_to_stdout: bool,
}

/// Opens the files a Redirects struct points to and returns Stdio handles
/// ready to hand to Command. Returns None if a file couldn't be opened,
/// after printing an error, so the caller can bail without spawning.
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

/// Opens a redirect target for writing, respecting append-vs-truncate.
/// Shared by the external-command path (which wraps this into a Stdio to
/// hand a spawned child) and the builtin-output path (which uses the
/// File as a Write target directly, since builtins run in-process and
/// never go through Stdio at all).
fn open_redirect_target(path: &str, append: bool) -> std::io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .append(append)
        .truncate(!append)
        .open(path)
}

// Creates a real anonymous pipe via pipe(2), returned as (read end,
// write end). std has no stable pipe constructor — Stdio::piped() makes
// one internally but keeps the write end private — and supporting
// `2>&1` inside a pipeline needs direct access to both ends so one
// write end can feed stdout AND stderr of the same child. Declared
// locally against libc (which std already links on every supported
// target) rather than pulling in nix or libc as a direct dependency,
// keeping the dependency list at exactly rustyline.
unsafe extern "C" {
    fn pipe(pipefd: *mut c_int) -> c_int;
}

/// Safe wrapper around the raw pipe(2) declaration above. Fails only if
/// the process is out of file descriptors.
fn create_os_pipe() -> io::Result<(File, File)> {
    let mut fds: [c_int; 2] = [0; 2];
    // SAFETY: pipe(2) writes at most two ints into the provided array.
    if unsafe { pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success both entries are fresh, valid, exclusively-owned
    // descriptors that nothing else in this single-threaded process will
    // touch; ownership moves into the two Files (and onward into Stdio /
    // the next pipeline stage).
    Ok(unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) })
}

fn open_redirect_stdout(redirects: &Redirects) -> Option<Stdio> {
    match &redirects.stdout {
        None => Some(Stdio::inherit()), // caller may override this for pipelines
        Some(path) => match open_redirect_target(path, redirects.append) {
            Ok(f) => Some(Stdio::from(f)),
            Err(e) => {
                eprintln!("lush: {}: {}", path, e);
                None
            }
        },
    }
}

/// Resolves the Stdio target for stderr, honoring `2>`, `2>>`, and
/// `2>&1`. For `2>&1`: if stdout has its own file redirect, stderr is
/// pointed at that same file (reopened by path, not a true
/// fd-duplicate, but behaviorally equivalent for a file target);
/// otherwise stderr inherits the terminal, which is where stdout is
/// headed in every case that reaches here — `2>&1` combined with stdout
/// flowing down a pipeline is resolved earlier by exec_pipeline's
/// hand-built shared pipe and never consults this function.
fn open_redirect_stderr(redirects: &Redirects) -> Option<Stdio> {
    if redirects.stderr_to_stdout {
        return match &redirects.stdout {
            Some(path) => match open_redirect_target(path, redirects.append) {
                Ok(f) => Some(Stdio::from(f)),
                Err(e) => {
                    eprintln!("lush: {}: {}", path, e);
                    None
                }
            },
            // stdout isn't going to a file, so it's inheriting the
            // terminal, and stderr goes to the same place.
            None => Some(Stdio::inherit()),
        };
    }

    match &redirects.stderr {
        None => Some(Stdio::inherit()),
        Some(path) => match open_redirect_target(path, redirects.stderr_append) {
            Ok(f) => Some(Stdio::from(f)),
            Err(e) => {
                eprintln!("lush: {}: {}", path, e);
                None
            }
        },
    }
}

/// Returns the writer a builtin's normal output should go to: the file
/// from a `>`/`>>` redirect on this command if one is present, otherwise
/// the shell's real stdout. Builtins run in-process rather than being
/// spawned, so there's no Stdio to hand them, they need an actual Write
/// target to print into directly. Boxed so both cases (File vs Stdout)
/// can be handled uniformly by the caller.
fn open_builtin_stdout(redirects: &Redirects) -> Option<Box<dyn Write>> {
    match &redirects.stdout {
        None => Some(Box::new(io::stdout())),
        Some(path) => match open_redirect_target(path, redirects.append) {
            Ok(f) => Some(Box::new(f)),
            Err(e) => {
                eprintln!("lush: {}: {}", path, e);
                None
            }
        },
    }
}

/// The stderr counterpart of open_builtin_stdout, honoring `2>`, `2>>`,
/// and `2>&1` with the same resolution rules as the external-command path:
/// an explicit stderr file wins; `2>&1` follows wherever stdout is headed
/// (the same file when stdout has one, otherwise the real terminal).
/// Builtins previously printed errors straight to the real stderr no
/// matter what, so `cd /nonexistent 2> err.log` silently lost its error —
/// this gives builtins real capture parity with external commands.
fn open_builtin_stderr(redirects: &Redirects) -> Option<Box<dyn Write>> {
    if redirects.stderr_to_stdout {
        return match &redirects.stdout {
            Some(path) => match open_redirect_target(path, redirects.append) {
                Ok(f) => Some(Box::new(f)),
                Err(e) => {
                    eprintln!("lush: {}: {}", path, e);
                    None
                }
            },
            None => Some(Box::new(io::stderr())),
        };
    }

    match &redirects.stderr {
        None => Some(Box::new(io::stderr())),
        Some(path) => match open_redirect_target(path, redirects.stderr_append) {
            Ok(f) => Some(Box::new(f)),
            Err(e) => {
                eprintln!("lush: {}: {}", path, e);
                None
            }
        },
    }
}

/// Commands that must run in-process rather than being spawned.
/// Note: cd/exit inside a pipeline still only affect the shell itself if
/// they're the *only* stage doing the work; a true subshell-per-stage
/// semantics is out of scope for now, this just stops them from being
/// treated as (nonexistent) external binaries.
fn is_builtin(cmd: &str) -> bool {
    matches!(
        cmd,
        "cd" | "exit" | "alias" | "unalias" | "source" | "." | "funcsave" | "export" | "unset"
            | "pwd"
    )
}

/// Runs any builtin except `exit`, which every caller handles separately
/// since it needs to unwind to the main loop rather than just return a
/// status code. `out` is where this builtin's normal (stdout) output goes,
/// real stdout normally, or a file when a `>`/`>>` redirect is present on
/// the command, letting things like `pwd > out.txt` and `alias > out.txt`
/// actually work. `err_out` mirrors that for stderr (`2>`, `2>>`,
/// `2>&1`), so builtin errors are capturable exactly like external
/// commands' — previously they always went straight to the real stderr.
fn run_builtin(cmd: &str, args: &[String], out: &mut dyn Write, err_out: &mut dyn Write) -> i32 {
    match cmd {
        "cd" => {
            let target = args.get(0).map(|s| s.as_str()).unwrap_or("~");

            // `cd -` jumps to $OLDPWD and, like bash, prints the
            // resulting directory to stdout, since it's not otherwise
            // visible, you didn't type the path yourself.
            let (path, print_after) = if target == "-" {
                match env::var("OLDPWD") {
                    Ok(old) => (old, true),
                    Err(_) => {
                        let _ = writeln!(err_out, "lush: cd: OLDPWD not set");
                        return 1;
                    }
                }
            } else {
                (expand_tilde(target), false)
            };

            // Capture the current directory before changing, this
            // becomes the new $OLDPWD, but only commit it (and the new
            // $PWD) after a successful chdir, a failed cd attempt
            // shouldn't touch either variable.
            let previous = env::current_dir().ok();

            match env::set_current_dir(Path::new(&path)) {
                Ok(()) => {
                    // SAFETY: lush is single-threaded, no other code in
                    // this process concurrently reads/writes the
                    // environment, which is exactly the condition
                    // set_var's unsafety exists to guard against.
                    unsafe {
                        if let Some(prev) = &previous {
                            env::set_var("OLDPWD", prev.as_os_str());
                        }
                        if let Ok(new_cwd) = env::current_dir() {
                            env::set_var("PWD", new_cwd.as_os_str());
                            if print_after {
                                let _ = writeln!(out, "{}", new_cwd.display());
                            }
                        }
                    }
                    0
                }
                Err(e) => {
                    let _ = writeln!(err_out, "lush: cd: {}: {}", path, e);
                    1
                }
            }
        }
        "pwd" => {
            // Reads through lookup_variable (shell-vars-then-environment,
            // same path $PWD expansion uses) rather than going straight
            // to the OS, so `pwd` and `echo $PWD` can never disagree,
            // even in the edge case of someone shadowing PWD with a bare
            // assignment. Falls back to asking the OS directly only if
            // $PWD somehow isn't set at all (shouldn't normally happen,
            // main() initializes it at startup).
            match lookup_variable("PWD") {
                Some(pwd) => {
                    let _ = writeln!(out, "{}", pwd);
                }
                None => match env::current_dir() {
                    Ok(cwd) => {
                        let _ = writeln!(out, "{}", cwd.display());
                    }
                    Err(e) => {
                        let _ = writeln!(err_out, "lush: pwd: {}", e);
                        return 1;
                    }
                },
            }
            0
        }
        "alias" => {
            if args.is_empty() {
                // No args: list all current aliases, alphabetically.
                let aliases = get_aliases().lock().unwrap();
                let mut names: Vec<&String> = aliases.keys().collect();
                names.sort();
                for name in names {
                    let _ = writeln!(out, "alias {}='{}'", name, aliases[name]);
                }
                return 0;
            }
            // Words got split on whitespace, so `alias ll="ls -la"` arrives
            // as one word ("ll=ls -la", quotes already handled by the
            // lexer) in the common case, but join defensively in case
            // someone wrote spaces around the '='.
            let full = args.join(" ");
            match full.find('=') {
                Some(eq_idx) => {
                    let name = full[..eq_idx].trim().to_string();
                    let value = full[eq_idx + 1..].trim().to_string();
                    if name.is_empty() {
                        let _ = writeln!(err_out, "lush: alias: invalid syntax, expected name=value");
                        1
                    } else {
                        get_aliases().lock().unwrap().insert(name, value);
                        0
                    }
                }
                None => {
                    let _ = writeln!(err_out, "lush: alias: invalid syntax, expected name=value");
                    1
                }
            }
        }
        "unalias" => match args.get(0) {
            Some(name) => {
                if get_aliases().lock().unwrap().remove(name).is_some() {
                    0
                } else {
                    let _ = writeln!(err_out, "lush: unalias: {}: not found", name);
                    1
                }
            }
            None => {
                let _ = writeln!(err_out, "lush: unalias: missing name");
                1
            }
        },
        "source" | "." => match args.get(0) {
            Some(path) => run_script_file(&expand_tilde(path), true),
            None => {
                let _ = writeln!(err_out, "lush: {}: missing filename", cmd);
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
                                let _ = writeln!(err_out, "lush: funcsave: $HOME not set");
                                return 1;
                            }
                        };
                        if let Err(e) = std::fs::create_dir_all(&dir) {
                            let _ = writeln!(err_out, "lush: funcsave: {}: {}", dir, e);
                            return 1;
                        }
                        let path = format!("{}/{}.lush", dir, name);
                        // Write in the same function/end format fish uses.
                        // --wraps guesses at the underlying command (first
                        // word of the expansion); $argv is added explicitly
                        // in the body so args pass through on invocation.
                        let wraps = value.split_whitespace().next().unwrap_or(name);
                        let contents = format!(
                            "function {} --wraps={} --description 'alias {}={}'\n    {} $argv\nend\n",
                            name, wraps, name, value, value
                        );
                        match std::fs::write(&path, contents) {
                            Ok(()) => {
                                let _ = writeln!(out, "Saved '{}' to {}", name, path);
                                0
                            }
                            Err(e) => {
                                let _ = writeln!(err_out, "lush: funcsave: {}: {}", path, e);
                                1
                            }
                        }
                    }
                    None => {
                        let _ = writeln!(err_out, "lush: funcsave: {}: no such alias defined", name);
                        1
                    }
                }
            }
            None => {
                let _ = writeln!(err_out, "lush: funcsave: missing name");
                1
            }
        },
        "export" => {
            if args.is_empty() {
                // No args: list currently-exported shell variables.
                let vars = get_shell_vars().lock().unwrap();
                let mut names: Vec<&String> = vars.keys().collect();
                names.sort();
                for name in names {
                    let _ = writeln!(out, "export {}={}", name, vars[name]);
                }
                return 0;
            }
            let mut status = 0;
            for arg in args {
                match arg.find('=') {
                    Some(eq_pos) => {
                        let name = &arg[..eq_pos];
                        let value = &arg[eq_pos + 1..];
                        if name.is_empty() {
                            let _ = writeln!(err_out, "lush: export: invalid name in '{}'", arg);
                            status = 1;
                            continue;
                        }
                        get_shell_vars()
                            .lock()
                            .unwrap()
                            .insert(name.to_string(), value.to_string());
                        // SAFETY: lush is single-threaded (no other code
                        // in this process is concurrently reading/writing
                        // the environment), which is exactly the
                        // condition set_var's unsafety exists to guard
                        // against.
                        unsafe {
                            env::set_var(name, value);
                        }
                    }
                    None => {
                        // `export NAME` with no '=': promote an existing
                        // shell-local variable to also be a real
                        // environment variable, matching bash.
                        let existing = get_shell_vars().lock().unwrap().get(arg).cloned();
                        match existing {
                            Some(value) => {
                                // SAFETY: see above, single-threaded.
                                unsafe {
                                    env::set_var(arg, value);
                                }
                            }
                            None => {
                                let _ = writeln!(err_out, "lush: export: {}: not set", arg);
                                status = 1;
                            }
                        }
                    }
                }
            }
            status
        }
        "unset" => {
            if args.is_empty() {
                let _ = writeln!(err_out, "lush: unset: missing name");
                return 1;
            }
            for arg in args {
                get_shell_vars().lock().unwrap().remove(arg);
                // SAFETY: see above, single-threaded.
                unsafe {
                    env::remove_var(arg);
                }
            }
            0
        }
        _ => unreachable!("run_builtin called with non-builtin: {}", cmd),
    }
}

/// Runs a parsed pipeline (replaces the old run_pipeline, now operating on
/// a slice of SimpleCommand from the AST instead of re-tokenizing raw
/// strings per stage). Behavior is unchanged: builtins mid-pipeline run
/// in-process, per-stage redirects override the pipe connection, and the
/// exit code is the last stage's status. One behavior upgrade over the
/// string-expansion era: `2>&1` on a middle stage now genuinely merges
/// stderr into the pipeline (via a hand-built shared pipe, see the
/// stdout/stderr resolution below) instead of silently dropping stderr
/// back to the terminal.
///
/// Alias handling per stage: a single-command alias dissolves into the
/// stage (its words and redirects replace the invocation's). An
/// operator-bearing alias can't participate in a pipeline at all — there
/// are no subshells to give a multi-command chain somewhere coherent to
/// live between two pipe stages — so that's a hard error rather than
/// silent misbehavior.
fn exec_pipeline(cmds: &[SimpleCommand], expand_aliases: bool) -> i32 {
    let mut children: Vec<std::process::Child> = Vec::new();
    let mut prev_stdout: Option<Stdio> = None;
    // Tracks the exit code when the last stage turns out to be a builtin
    // (which never becomes a Child, so it can't be read off `children`).
    let mut last_builtin_status: Option<i32> = None;

    for (i, stage) in cmds.iter().enumerate() {
        // Resolve a possible alias on this stage's first word before
        // anything else looks at it. A single-command expansion replaces
        // both the words AND contributes its own redirects ahead of the
        // site's (redirect folding is last-one-wins per kind, so the
        // site's redirect still overrides — the same precedence typed
        // input would produce).
        let mut stage_words = stage.words.clone();
        let mut stage_redirects = stage.redirects.clone();
        if expand_aliases {
            if let Some(first) = stage_words.first() {
                let name = first.text();
                if let Some(value) = get_aliases().lock().unwrap().get(&name).cloned() {
                    match parse(&value) {
                        Ok(Some(Node::Command(mut expanded))) => {
                            // Single-command alias: dissolve into the stage.
                            // Args follow the same $argv-or-append rule as a
                            // standalone invocation; the value's own
                            // redirects come first so the site's still win
                            // per-kind during folding.
                            insert_args_into_command(&mut expanded, &stage_words[1..]);
                            expanded.redirects.extend(stage.redirects.iter().cloned());
                            stage_words = expanded.words;
                            stage_redirects = expanded.redirects;
                        }
                        Ok(Some(_)) => {
                            eprintln!(
                                "lush: alias '{}': aliases containing operators can't be used inside a pipeline yet",
                                name
                            );
                            reap(children);
                            return 1;
                        }
                        Ok(None) => {
                            eprintln!("lush: alias '{}': expands to nothing", name);
                            reap(children);
                            return 1;
                        }
                        Err(e) => {
                            eprintln!("lush: alias '{}': {}", name, e);
                            reap(children);
                            return 1;
                        }
                    }
                }
            }
        }

        let expanded_words = stage_words;
        let words = expand_command_words(&expanded_words);
        if words.is_empty() {
            eprintln!("lush: syntax error: missing command in pipeline");
            reap(children);
            return 1;
        }
        let cmd = words[0].as_str();
        let args = &words[1..];
        let is_last = i == cmds.len() - 1;
        let redirects = redirects_from_ast(&stage_redirects);
        if is_builtin(cmd) {
            // Builtins can't be spawned as a child process, so they can't
            // participate in the pipe chain the normal way. Run them
            // in-process; if they're not the last stage there's no
            // meaningful output to hand downstream, so just skip forward.
            // A `>`/`>>` redirect on this stage still works (writes to
            // the file instead), it just can't feed the next pipeline
            // stage, that's a separate, bigger feature not covered here.
            let status = match cmd {
                "exit" => {
                    reap(children);
                    return exit_requested(args);
                }
                _ => match (
                    open_builtin_stdout(&redirects),
                    open_builtin_stderr(&redirects),
                ) {
                    (Some(mut out), Some(mut err)) => {
                        run_builtin(cmd, args, out.as_mut(), err.as_mut())
                    }
                    _ => 1, // a redirect target failed to open; error already printed
                },
            };
            last_builtin_status = Some(status);
            prev_stdout = None;
            continue;
        }
        last_builtin_status = None; // this stage is external, overrides any earlier builtin

        // An explicit `< file` on this stage wins over whatever the previous
        // stage would have piped in (matches typical shell behavior, this
        // stage just ignores upstream data and reads the file instead).
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

        // An explicit `> file` / `>> file` on this stage wins over piping to
        // the next stage. This mirrors real shells: `cmd1 | cmd2 > out.txt`
        // sends cmd2's output to the file, not to a (nonexistent) cmd3.
        let stdout_is_piped = redirects.stdout.is_none() && !is_last;

        // `2>&1` while stdout is flowing down the pipe needs both
        // descriptors to point at the SAME write end. Stdio::piped()
        // can't express that — it builds a private pipe whose write end
        // never surfaces — so in exactly that case the pipe is built by
        // hand: one write end feeds both stdout and stderr of this child,
        // and the read end becomes the next stage's stdin, indistinguishable
        // from a std-created pipe downstream.
        let merge_stderr_into_pipe = stdout_is_piped && redirects.stderr_to_stdout;
        let mut manual_read_end: Option<File> = None;

        let (stdout, stderr) = if merge_stderr_into_pipe {
            match create_os_pipe() {
                Ok((read_end, write_end)) => match write_end.try_clone() {
                    Ok(write_dup) => {
                        manual_read_end = Some(read_end);
                        (Stdio::from(write_end), Stdio::from(write_dup))
                    }
                    Err(e) => {
                        eprintln!("lush: 2>&1: {}", e);
                        reap(children);
                        return 1;
                    }
                },
                Err(e) => {
                    eprintln!("lush: 2>&1: {}", e);
                    reap(children);
                    return 1;
                }
            }
        } else {
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

            let stderr = match open_redirect_stderr(&redirects) {
                Some(s) => s,
                None => {
                    reap(children);
                    return 1;
                }
            };
            (stdout, stderr)
        };

        let mut child = match Command::new(cmd)
            .args(args)
            .stdin(stdin)
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("lush: {}: {}", cmd, e);
                // Don't leave earlier stages running with nowhere for their
                // output to go, wait on what's already spawned before bailing.
                reap(children);
                return 127;
            }
        };

        // When this stage's output was piped by hand (2>&1 merge), the read
        // end is already in our possession; otherwise std created the pipe
        // and hands us the read end via the child. Either way it becomes
        // the next stage's stdin.
        prev_stdout = match manual_read_end.take() {
            Some(read_end) => Some(Stdio::from(read_end)),
            None => child.stdout.take().map(Stdio::from),
        };
        children.push(child);
    }

    let last_child_status = reap_and_get_last(children);
    // If the final stage was a builtin, its status takes precedence, it
    // never went through `children` in the first place.
    last_builtin_status.unwrap_or(last_child_status)
}

/// Waits on every child in the pipeline so none are left as zombies,
/// even if an earlier stage in the chain failed. Discards exit codes,
/// used on error paths where a fixed status is returned instead.
fn reap(children: Vec<std::process::Child>) {
    for mut child in children {
        let _ = child.wait();
    }
}

/// Waits on every child, same as `reap`, but returns the exit code of the
/// last one in the list (i.e. the last external stage in the pipeline).
/// Returns 0 if the list is empty (e.g. a pipeline that was entirely
/// builtins, though that's an unusual edge case).
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

#[cfg(test)]
mod alias_expansion_tests {
    use super::*;
    use lush::WordPart;

    fn word(s: &str) -> Word {
        Word {
            parts: vec![WordPart::Expandable(s.to_string())],
        }
    }

    fn words(v: &[&str]) -> Vec<Word> {
        v.iter().map(|s| word(s)).collect()
    }

    /// Parses input, panics on failure — tests only exercise known-good
    /// syntax here.
    fn parse_node(input: &str) -> Node {
        parse(input).ok().flatten().expect("test parse failed")
    }

    /// Renders a command's words for assertions.
    fn cmd_words(cmd: &SimpleCommand) -> Vec<String> {
        cmd.words.iter().map(|w| w.text()).collect()
    }

    #[test]
    fn argv_splices_at_first_occurrence_across_chain() {
        let mut node = parse_node("echo pre $argv && echo post");
        assert!(try_splice_argv(&mut node, &words(&["X", "Y"])));
        let Node::And(lhs, _) = &node else {
            panic!("expected And");
        };
        let Node::Command(cmd) = lhs.as_ref() else {
            panic!("expected Command");
        };
        assert_eq!(cmd_words(cmd), vec!["echo", "pre", "X", "Y"]);
    }

    #[test]
    fn argv_absent_leaves_words_untouched() {
        let mut node = parse_node("echo a && echo b");
        assert!(!try_splice_argv(&mut node, &words(&["X"])));
        let Node::And(_, rhs) = &node else {
            panic!("expected And");
        };
        let Node::Command(cmd) = rhs.as_ref() else {
            panic!("expected Command");
        };
        assert_eq!(cmd_words(cmd), vec!["echo", "b"]);
    }

    #[test]
    fn args_append_to_last_command_of_pipeline_inside_sequence() {
        // two='echo one ; cat | grep' + args → args land on grep.
        let node = parse_node("echo one ; cat | grep");
        let node = insert_args_into_node(node, &words(&["Z"]));
        let Node::Sequence(_, rhs) = node else {
            panic!("expected Sequence");
        };
        let Node::Pipeline(stages) = *rhs else {
            panic!("expected Pipeline");
        };
        assert_eq!(cmd_words(&stages[1]), vec!["grep", "Z"]);
    }

    #[test]
    fn argv_in_pipeline_stage_wins_over_appending() {
        let mut node = parse_node("cat | grep $argv | wc");
        assert!(try_splice_argv(&mut node, &words(&["pat"])));
        let Node::Pipeline(stages) = &node else {
            panic!("expected Pipeline");
        };
        assert_eq!(cmd_words(&stages[0]), vec!["cat"]);
        assert_eq!(cmd_words(&stages[1]), vec!["grep", "pat"]);
        assert_eq!(cmd_words(&stages[2]), vec!["wc"]);
    }

    #[test]
    fn site_redirects_land_on_rightmost_leaf() {
        // gs > out.txt where gs='echo a && echo b': the redirect attaches
        // to `echo b`, mirroring textual substitution order.
        let mut node = parse_node("echo a && echo b");
        attach_redirects_to_last_command(
            &mut node,
            &[AstRedirect::Out(word("out.txt"))],
        );
        let Node::And(_, rhs) = &node else {
            panic!("expected And");
        };
        let Node::Command(cmd) = rhs.as_ref() else {
            panic!("expected Command");
        };
        assert_eq!(
            cmd.redirects,
            vec![AstRedirect::Out(word("out.txt"))],
        );
    }

    #[test]
    fn full_invocation_appends_args_and_redirects() {
        let node = parse_node("echo status && echo diff");
        let node = insert_args_into_node(node, &words(&["extra"]));
        let mut node = node;
        attach_redirects_to_last_command(&mut node, &[AstRedirect::Append(word("log.txt"))]);
        let Node::And(lhs, rhs) = &node else {
            panic!("expected And");
        };
        let Node::Command(left) = lhs.as_ref() else {
            panic!("expected Command");
        };
        let Node::Command(right) = rhs.as_ref() else {
            panic!("expected Command");
        };
        assert_eq!(cmd_words(left), vec!["echo", "status"]);
        assert!(left.redirects.is_empty());
        assert_eq!(cmd_words(right), vec!["echo", "diff", "extra"]);
        assert_eq!(right.redirects.len(), 1);
    }

    #[test]
    fn invocation_of_unparseable_value_is_an_error() {
        let result = build_alias_invocation("bad", "echo hi >", &[], &[]);
        match result {
            Err(msg) => assert!(msg.contains("alias 'bad'")),
            Ok(_) => panic!("expected parse error"),
        }
    }

    #[test]
    fn invocation_of_empty_value_is_a_noop() {
        assert_eq!(
            build_alias_invocation("empty", "   ", &[], &[]),
            Ok(None)
        );
    }

    #[test]
    fn single_command_invocation_parses_with_quoting_intact() {
        // Quoting inside an alias's value survives: "hi there" stays one word.
        let result = build_alias_invocation("msg", r#"echo "hi there""#, &[], &[]);
        let Some(Node::Command(cmd)) = result.ok().flatten() else {
            panic!("expected single Command");
        };
        assert_eq!(cmd_words(&cmd), vec!["echo", "hi there"]);
    }
}
