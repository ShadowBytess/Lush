
# Lush




A custom Unix shell for Linux, written from scratch in Rust. Built for CachyOS, aiming to feel like a genuinely usable daily-driver shell with its own identity, not a Bash or Fish clone.




> **Status: early development.** Lush works well enough for interactive use and testing, but is not yet recommended as anyone's only login shell. See [Known bugs & limitations](#known-bugs--limitations) before running `chsh`.




## Features




### Working today




- Command chaining: `&&`, `||`, `;` with proper short-circuit evaluation

- Pipelines: `cmd1 | cmd2 | cmd3`

- Redirection: `<`, `>`, `>>`, `2>`, `2>>`, `2>&1`, `&>` — including `2>&1` genuinely merging stderr into a pipeline

- Variable expansion: `$VAR` / `${VAR}` (suppressed inside single quotes), shell variables via bare `name=value` assignments; an unquoted expansion that comes out empty vanishes from the command's arguments, bash-style (`echo A $UNSET B` prints `A B`)

- Filename globbing: `*`, `?`, `[abc]`, `[a-z]`, `[!...]`, including subdirectory patterns (`src/*.rs`) and trailing `/` for dirs-only; dotfiles need an explicit leading `.`; unmatched patterns stay literal; quoted wildcards never expand

- Aliases: `alias name=value`, `unalias name`, listing with bare `alias`; alias values may contain full operator chains (`&&`, `||`, `;`, pipes, redirects) and are executed as real parsed syntax, with fish-style `$argv` argument insertion

- Fish-style function files: `~/.config/lush/functions/<name>.lush`, using `function name --wraps=... --description '...'` / body / `end` syntax, with `$argv` for argument passthrough

- `funcsave name` — save a session alias out to its own function file

- `source path` / `. path` — re-run a script file on demand

- `~/.lushrc` — startup config, runs through the same execution path as interactive input

- Builtins accept output redirection: stdout (`pwd > out.txt`) and stderr (`cd /nonexistent 2> err.log`, including `2>>` and `2>&1`)

- `exit [n]` unwinds cleanly — history is saved and the process exits with status `n` (0 by default)

- `cd -` jumps to the previous directory via `$OLDPWD`; `$PWD`/`$OLDPWD` are tracked

- Implicit `cd` — typing a bare path (`..`, `../..`, `subdir/child`, `~/Projects`) changes into it directly, no `cd` needed

- Tab completion for files and directories (via `rustyline`)

- Fish-style inline history autosuggestions (dim ghost text, accept with `→`/`End`)

- Persistent command history (`~/.lush_history`)

- Exit status tracking and propagation through chains and pipelines

- Quoting: both `'single'` and `"double"` quotes




### Builtins




`cd`, `pwd`, `exit`, `alias`, `unalias`, `source` / `.`, `funcsave`, `export`, `unset`




## Known bugs & limitations




These are real, currently-present gaps, not hypothetical edge cases:




- **No backslash escaping.** There's no way to escape a quote or space character within a word.

- **Operator-bearing aliases can't run inside a pipeline.** `alias gs='git status && git diff'` works fine standalone, but `gs | head` is rejected with an explicit error — there are no subshells yet to give a multi-command chain somewhere coherent to live between two pipe stages.

- **No `history` builtin.** History is recorded and used for autosuggestions, but can't be listed or searched from within the shell beyond whatever `rustyline` provides by default.

- **No `help`, `type`, `which`, or `command -v`.**

- **Tab completion is filename-only.** It doesn't know about builtins, aliases, functions, or `$PATH` executables yet.

- **Alias expansion is one level deep.** An alias can't reference another alias (this is intentional for now, to avoid `alias ll=ll`-style infinite loops, but it does mean alias composition doesn't work).

- **Implicit `cd` doesn't cover bare directory names.** `Documents` (no slash) won't auto-`cd` even if that directory exists, only `..`, `~...`, and anything containing `/` do. This is deliberate: a bare word is ambiguous with a real command of the same name, and there's no reliable cross-platform way to fall back to `cd` only after command lookup fails.

- **No job control.** No background execution (`&`), no `jobs`/`fg`/`bg`, no `Ctrl+Z` suspend.

- **No `pipefail` option.** Pipeline exit status is always the last stage's status.

- **Functions are really just multi-word aliases**, not parsed/executed shell syntax. No control flow inside a function body.

- **No scripting constructs.** No `if`, `for`, `while`.

- **No command substitution.** `$(...)` is not supported.

- **Single-quote handling can surprise you mid-sentence.** Since `'` now opens a real quote (needed to parse `--description '...'` in function files), an unescaped apostrophe — e.g. `echo don't stop` — will swallow the rest of the line into one token rather than erroring, the same way bash behaves in that situation. Worth knowing if output looks wrong on a command containing a contraction.

- **No signal handling beyond the default.** `Ctrl+C` interrupts the current line but there's no custom `SIGINT`/`SIGTSTP` handling for child processes yet.




## Architecture




Lush runs on a real lexer/parser/AST pipeline, end to end:




```

Input → Lexer → Parser → AST → Executor

```




Every input line — typed interactively, sourced from a script, loaded from `~/.lushrc`, even an alias's own value at invocation time — goes through the same path. `lex()` tokenizes quote-aware (a `|` inside quotes stays literal text), `parse()` builds an AST (`Command` / `Pipeline` / `And` / `Or` / `Sequence`, with per-command redirects), and the executor in `src/main.rs` walks that tree, propagating exit statuses through short-circuit chains and pipelines.

Split of responsibilities:

- `src/lib.rs` — lexer, parser, AST types, variable expansion (`expand_word`), assignment detection (`parse_assignment`). All pure functions, directly unit-tested.
- `src/main.rs` — the executor (`exec_node` / `exec_simple_command` / `exec_pipeline`), builtins, alias expansion, prompt/history/line-editor glue.

Alias expansion is AST-level too: invoking an alias parses its value into a subtree that replaces the invocation in the tree (arguments spliced fish-style at `$argv`, or appended to the chain's last command; site redirects land on that same last command). That's what makes operator-bearing aliases work; it's also why they're one-level only by construction — the expanded subtree executes with alias lookup disabled — and why an operator-bearing alias can't be invoked *inside* a pipeline (no subshells yet), which errors loudly instead of misbehaving quietly.




The AST shape:




```rust

enum Node {

    Command(SimpleCommand),

    Pipeline(Vec<SimpleCommand>),

    And(Box<Node>, Box<Node>),

    Or(Box<Node>, Box<Node>),

    Sequence(Box<Node>, Box<Node>),

}



struct SimpleCommand {

    words: Vec<Word>,

    redirects: Vec<Redirect>,

}

```




## Configuration




### `~/.lushrc`




Runs at startup. Supports `#` comments, blank lines, and any valid Lush syntax (since it's executed through the same path as interactive input):




```sh

# ~/.lushrc

alias ll="ls -la"

alias gs="git status"

echo "lush loaded"

```




### `~/.config/lush/functions/`




Fish-style function autoloading. Every `<name>.lush` file becomes an alias called `<name>`, loaded eagerly at startup (unlike fish's lazy per-call loading):




```sh

# ~/.config/lush/functions/btctl.lush

function btctl --wraps=bluetoothctl --description 'alias btctl=bluetoothctl'

    bluetoothctl $argv

end

```




Files without a `function ... end` block fall back to the legacy plain-text format (every non-comment line joined with spaces).




### History




Stored at `~/.lush_history`, loaded on startup and saved on exit.




## Building




```bash

cargo build --release

```




## Running without changing your login shell




Point your terminal emulator at the built binary directly (`target/release/lush` or wherever you've installed it), most terminal emulators support this per-profile. This is the recommended way to use Lush day to day until it's more mature.




## Installing as a login shell




Not recommended yet for a primary account — see [Known bugs & limitations](#known-bugs--limitations). If testing on a dedicated account:




```bash

cargo build --release

sudo cp target/release/lush /usr/local/bin/lush

sudo chmod +x /usr/local/bin/lush

echo /usr/local/bin/lush | sudo tee -a /etc/shells

chsh -s /usr/local/bin/lush

```




Keep a fallback login path that doesn't depend on Lush working (e.g. `su - youraccount -s /usr/bin/fish`), in case a broken build can't reach an interactive prompt.




## Testing




```bash

cargo test

```




Three layers, all run by `cargo test`:

- Lexer/parser/expansion unit tests in `src/lib.rs`
- Alias tree-manipulation unit tests in `src/main.rs`
- A black-box integration suite in `tests/integration.rs` that spawns the built binary with an isolated `$HOME`, feeds it scripted stdin, and asserts on stdout/stderr/exit codes

Add regression tests for parser behavior before relying on it in the executor, per the project's own engineering guidelines.




## Roadmap




Rough phase breakdown, in priority order. Not all of this will land quickly, and each phase depends on the one before it (in particular, most of Phase 2 onward is blocked on the lexer/parser/AST work in Phase 1 landing first).




- **Phase 1 — Foundation** *(complete)*: lexer/parser/AST, port existing syntax onto it, parser tests, verify no regressions

- **Phase 2 — Shell fundamentals** *(nearly complete — remaining: builtin stderr redirection, temporary per-command env assignments)*: `$VAR`/`${VAR}` expansion, shell variables, `export`/`unset`, `pwd`, `cd -`, `$PWD`/`$OLDPWD`, proper redirection parsing (including `2>`, `&>`), builtin redirection

- **Phase 3 — Interactive quality**: `history`, `help`, `type`, `which`/`command -v`, completion aware of builtins/aliases/functions/`$PATH`, better history search (`Ctrl+R`, prefix-aware), configurable prompt, optional git-aware prompt segment

- **Phase 4 — Process management**: background jobs (`&`), `jobs`/`fg`/`bg`, `Ctrl+Z`, proper signal/process-group handling, `pipefail`

- **Phase 5 — Scripting**: real function bodies (parsed shell syntax, not string aliases), glob expansion, command substitution, `if`/`for`/`while`

- **Phase 6 — Lush identity**: `mkcd`, directory bookmarks (`jump`), project-root navigation (`croot`), `trash`, `extract`, configurable themes, possibly a plugin architecture




### Lush-specific ideas (not in other shells, or not by default)




- `mkcd <dir>` — create a directory and `cd` into it in one step

- `jump <bookmark>` — jump to a saved directory, bookmarks persisted under `~/.config/lush/`

- `croot` — walk upward from cwd until a project marker (`.git`, `Cargo.toml`, `package.json`, `CMakeLists.txt`) is found, then `cd` there

- `trash <file>` — move to the desktop trash instead of deleting outright

- `extract <archive>` — detect and extract common archive formats without remembering the right flags per format




## Engineering principles




These guide how the codebase evolves:




1. Don't rewrite working code without a reason.

2. Correctness over feature count.

3. Prefer a proper lexer/parser/AST over more `split()` calls.

4. Keep parsing separate from execution.

5. Keep builtins separate from external command spawning.

6. Minimize dependencies (currently just `rustyline`).

7. Unix/Linux-first; no Windows support planned.

8. Handle errors, don't panic.

9. Preserve existing behavior; explain any deliberate behavior change.

10. `cargo check` (and `cargo test` where applicable) after substantial changes.

11. Parser changes get tests before the executor relies on them.

12. Incremental, reviewable changes over large batched ones.


