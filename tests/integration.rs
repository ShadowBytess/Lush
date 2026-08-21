//! Black-box integration tests: drive the actual lush binary with scripted
//! stdin and assert on its stdout/stderr/exit code.
//!
//! Each test gets a throwaway $HOME (fresh temp dir, also used as cwd) so
//! real user config (~/.lushrc, function files, history) can never leak in,
//! and any files the shell writes stay out of the repo.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Sandbox {
    home: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let home = std::env::temp_dir().join(format!(
            "lush-it-{}-{}-{}",
            name,
            std::process::id(),
            id
        ));
        fs::create_dir_all(&home).expect("create sandbox home");
        Sandbox { home }
    }

    /// Feeds `input` to one lush process, closes its stdin (EOF), and
    /// collects the result. The shell exits on EOF by itself.
    fn run(&self, input: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lush"))
            .env("HOME", &self.home)
            .current_dir(&self.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn lush");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write input");
        child.wait_with_output().expect("wait for lush")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.home);
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[track_caller]
fn assert_exit_ok(out: &Output) {
    assert_eq!(
        out.status.code(),
        Some(0),
        "lush exited abnormally\nstdout:\n{}\nstderr:\n{}",
        stdout(out),
        stderr(out)
    );
}

#[test]
fn runs_simple_command() {
    let sb = Sandbox::new("simple");
    let out = sb.run("echo hello\n");
    assert_exit_ok(&out);
    assert_eq!(stdout(&out).trim_end(), "hello");
}

#[test]
fn quoted_pipe_stays_literal() {
    // Regression guard for the original motivating bug of the whole
    // lexer migration: a pipe inside quotes is text, not an operator.
    let sb = Sandbox::new("quoted-pipe");
    let out = sb.run("echo \"hello | world\"\n");
    assert_exit_ok(&out);
    assert!(stdout(&out).contains("hello | world"));
}

#[test]
fn glued_redirection_needs_no_whitespace() {
    let sb = Sandbox::new("glued-redirect");
    let out = sb.run("echo hi>f.txt\ncat f.txt\n");
    assert_exit_ok(&out);
    assert!(stdout(&out).trim_end().ends_with("hi"));
}

#[test]
fn variables_expand_unless_single_quoted() {
    let sb = Sandbox::new("vars");
    let out = sb.run("name=world\necho hello $name\necho '${name}'\n");
    assert_exit_ok(&out);
    let out_str = stdout(&out);
    assert!(out_str.contains("hello world"), "got: {out_str}");
    assert!(out_str.contains("${name}"), "got: {out_str}");
}

#[test]
fn chains_short_circuit_correctly() {
    let sb = Sandbox::new("chains");
    let out = sb.run("false && echo yes || echo fallback\ntrue && echo reached\n");
    assert_exit_ok(&out);
    let out_str = stdout(&out);
    assert!(!out_str.contains("yes"));
    assert!(out_str.contains("fallback"));
    assert!(out_str.contains("reached"));
}

#[test]
fn operator_alias_runs_full_chain() {
    // The headline fix of this migration round: operators inside an
    // alias's value used to be silently dropped.
    let sb = Sandbox::new("alias-chain");
    let out = sb.run("alias gs='echo status && echo diff'\ngs\n");
    assert_exit_ok(&out);
    let out_str = stdout(&out);
    assert!(out_str.contains("status"), "got: {out_str}");
    assert!(out_str.contains("diff"), "got: {out_str}");
    assert!(
        out_str.find("status").unwrap() < out_str.find("diff").unwrap(),
        "chain order wrong: {out_str}"
    );
}

#[test]
fn alias_args_splice_into_argv() {
    let sb = Sandbox::new("alias-argv");
    let out = sb.run("alias w='echo pre $argv post'\nw A B\n");
    assert_exit_ok(&out);
    assert!(stdout(&out).contains("pre A B post"));
}

#[test]
fn alias_trailing_args_append_to_last_command() {
    // two='echo one && echo tail' + extra → args land after the LAST
    // command of the chain (textual-substitution order), not the first.
    let sb = Sandbox::new("alias-append");
    let out = sb.run("alias two='echo one && echo tail'\ntwo extra\n");
    assert_exit_ok(&out);
    let out_str = stdout(&out);
    assert!(out_str.contains("tail extra"), "got: {out_str}");
    assert!(!out_str.contains("one extra"), "args went to wrong command");
}

#[test]
fn site_redirect_applies_to_operator_alias() {
    // gs > out.txt where gs='echo a && echo b': the redirect belongs to
    // the LAST command of the expansion, so `echo a` still goes to the
    // terminal while only "b" lands in the file.
    let sb = Sandbox::new("alias-redirect");
    let out = sb.run("alias gs='echo a && echo b'\ngs > o.txt\n");
    assert_exit_ok(&out);
    let out_str = stdout(&out);
    assert!(out_str.contains("a"), "`echo a` should hit the terminal: {out_str}");
    let file = fs::read_to_string(sb.home.join("o.txt")).expect("o.txt exists");
    assert_eq!(file, "b\n");
}

#[test]
fn operator_alias_inside_pipeline_hard_errors() {
    // No subshell semantics yet: invoking an operator-bearing alias as a
    // pipeline stage must fail loudly rather than misbehave quietly.
    let sb = Sandbox::new("alias-in-pipe");
    let out = sb.run("alias gs='echo alpha && echo beta'\ngs | cat\n");
    assert_exit_ok(&out); // the SHELL survives; the command failed
    assert!(
        stderr(&out).contains("can't be used inside a pipeline"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(
        !stdout(&out).contains("alpha") && !stdout(&out).contains("beta"),
        "alias ran despite pipeline error"
    );
}

#[test]
fn simple_alias_inside_pipeline_still_works() {
    // Single-command aliases dissolve into their pipeline stage as before.
    let sb = Sandbox::new("alias-simple-pipe");
    let out = sb.run("alias up='tr a-z A-Z'\necho mixed | up\n");
    assert_exit_ok(&out);
    assert!(stdout(&out).contains("MIXED"));
}

#[test]
fn stderr_joins_pipeline_via_2_to_1() {
    // The other headline fix: 2>&1 on a middle stage merges stderr into
    // the pipe instead of dropping it to the terminal.
    let sb = Sandbox::new("stderr-pipe");
    let out = sb.run("/bin/sh -c 'echo out; echo err >&2' 2>&1 | grep -c .\n");
    assert_exit_ok(&out);
    assert_eq!(stdout(&out).trim_end().chars().last(), Some('2'));
}

#[test]
fn builtin_stdout_redirect_works() {
    let sb = Sandbox::new("builtin-redirect");
    let home = sb.home.to_string_lossy().to_string();
    let out = sb.run("pwd > pf.txt\ncat pf.txt\n");
    assert_exit_ok(&out);
    assert!(
        stdout(&out).contains(&home),
        "expected cwd in output: {}",
        stdout(&out)
    );
}

#[test]
fn funcsave_persists_across_restarts() {
    // Two separate processes: define + funcsave in the first, invoke from
    // the autoloaded function file in the second.
    let sb = Sandbox::new("funcsave");
    let first = sb.run("alias ft='echo saved'\nfuncsave ft\n");
    assert_exit_ok(&first);
    assert!(stderr(&first).is_empty(), "stderr: {}", stderr(&first));
    let second = sb.run("ft\n");
    assert_exit_ok(&second);
    assert!(
        stdout(&second).contains("saved"),
        "function file not loaded: {}",
        stdout(&second)
    );
}

#[test]
fn dangling_redirect_reports_error_without_crashing() {
    let sb = Sandbox::new("dangling");
    let out = sb.run("echo hi >\necho after\n");
    assert_exit_ok(&out);
    assert!(!stderr(&out).is_empty());
    assert!(stdout(&out).contains("after"));
}

// ---------------------------------------------------------------------
// Filename globbing
// ---------------------------------------------------------------------

#[test]
fn rm_with_glob_removes_matching_files() {
    // The exact scenario that prompted glob support: `rm *.txt` used to
    // hand rm a literal "*.txt" and fail with "No such file or directory".
    let sb = Sandbox::new("rm-glob");
    fs::write(sb.home.join("a.txt"), "").unwrap();
    fs::write(sb.home.join("b.txt"), "").unwrap();
    fs::write(sb.home.join("keep.log"), "").unwrap();
    let out = sb.run("rm *.txt\n/bin/echo remaining: *.txt\n");
    assert_exit_ok(&out);
    assert!(!sb.home.join("a.txt").exists());
    assert!(!sb.home.join("b.txt").exists());
    assert!(sb.home.join("keep.log").exists());
    // Post-rm, the pattern matches nothing, so echo receives it literally.
    let out_str = stdout(&out);
    assert!(
        out_str.contains("remaining: *.txt"),
        "got: {out_str}"
    );
}

#[test]
fn quoted_glob_never_expands_even_when_matching() {
    let sb = Sandbox::new("glob-quoted");
    fs::write(sb.home.join("a.txt"), "").unwrap();
    let out = sb.run("/bin/echo \"*.txt\"\n");
    assert_exit_ok(&out);
    assert_eq!(stdout(&out).trim_end(), "*.txt");
}

#[test]
fn variable_holding_a_pattern_globs_like_bash() {
    let sb = Sandbox::new("glob-var");
    fs::write(sb.home.join("x.dat"), "").unwrap();
    fs::write(sb.home.join("y.dat"), "").unwrap();
    let out = sb.run("pat=*.dat\n/bin/echo $pat\n");
    assert_exit_ok(&out);
    let out_str = stdout(&out);
    assert!(out_str.contains("x.dat") && out_str.contains("y.dat"), "got: {out_str}");
}

// ---------------------------------------------------------------------
// exit builtin: unwinds to the main loop instead of process::exit
// ---------------------------------------------------------------------

#[test]
fn exit_stops_execution_but_shell_shuts_down_cleanly() {
    let sb = Sandbox::new("exit-stops");
    let out = sb.run("echo one\nexit\necho never\n");
    assert_eq!(out.status.code(), Some(0));
    let out_str = stdout(&out);
    assert!(out_str.contains("one"));
    assert!(!out_str.contains("never"), "ran past exit: {out_str}");
}

#[test]
fn exit_persists_session_history() {
    // The headline regression: `exit` used to process::exit(0) directly,
    // bypassing save_history and silently discarding the whole session.
    let sb = Sandbox::new("exit-history");
    sb.run("echo alpha\nexit\n");
    let history = fs::read_to_string(sb.home.join(".lush_history"))
        .expect("history must exist after exit");
    assert!(history.contains("alpha"), "history: {history}");
}

#[test]
fn exit_propagates_numeric_code() {
    let sb = Sandbox::new("exit-code");
    let out = sb.run("exit 3\n");
    assert_eq!(out.status.code(), Some(3));
}

#[test]
fn sequence_halts_at_exit() {
    let sb = Sandbox::new("exit-seq");
    let out = sb.run("echo before ; exit ; echo after\n");
    let out_str = stdout(&out);
    assert!(out_str.contains("before"));
    assert!(!out_str.contains("after"), "sequence ran past exit");
}

#[test]
fn exit_inside_pipeline_unwinds_cleanly() {
    let sb = Sandbox::new("exit-pipe");
    // `exit` as a non-first pipeline stage must stop the whole chain via
    // the sentinel path (reaping children first), not kill the process
    // mid-walk — the clean shutdown (history save) still happens.
    let out = sb.run("echo hi | exit\n");
    assert_eq!(out.status.code(), Some(0));
}

// ---------------------------------------------------------------------
// Empty-word handling (bash parity)
// ---------------------------------------------------------------------

#[test]
fn unset_variable_vanishes_from_argv() {
    let sb = Sandbox::new("empty-unset");
    let out = sb.run("/bin/echo A $NOPE B\n");
    assert_exit_ok(&out);
    assert_eq!(stdout(&out).trim_end(), "A B");
}

#[test]
fn quoted_empty_argument_survives() {
    let sb = Sandbox::new("empty-quoted");
    let out = sb.run("/bin/echo A \"\" B\n");
    assert_exit_ok(&out);
    // echo joins args with single spaces, so an empty middle arg shows
    // up as two consecutive spaces.
    assert!(stdout(&out).contains("A  B"), "got: {:?}", stdout(&out));
}

#[test]
fn ls_with_unset_variable_lists_directory() {
    // The practical case behind empty-word dropping: `ls $UNSET` used to
    // become `ls ""` and error.
    let sb = Sandbox::new("empty-ls");
    fs::write(sb.home.join("visible.txt"), "").unwrap();
    let out = sb.run("ls $NOPE\n");
    assert_exit_ok(&out);
    assert!(
        !stderr(&out).contains("No such file"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(stdout(&out).contains("visible.txt"));
}

// ---------------------------------------------------------------------
// Builtin stderr redirection
// ---------------------------------------------------------------------

#[test]
fn builtin_error_is_captured_by_stderr_redirect() {
    let sb = Sandbox::new("builtin-2>");
    let out = sb.run("cd /definitely-not-here 2> e.txt\necho moved-on\n");
    assert_exit_ok(&out);
    let captured = fs::read_to_string(sb.home.join("e.txt")).expect("e.txt must exist");
    assert!(captured.contains("lush: cd:"), "captured: {captured}");
    assert!(stdout(&out).contains("moved-on"));
}

#[test]
fn pwd_stderr_redirect_creates_empty_file() {
    // Even with nothing to write, the redirect target must be created —
    // previously builtins ignored 2> entirely.
    let sb = Sandbox::new("builtin-pwd-2>");
    let out = sb.run("pwd 2> e.txt\n");
    assert_exit_ok(&out);
    let captured = fs::read_to_string(sb.home.join("e.txt")).expect("e.txt must exist");
    assert!(captured.is_empty());
}
