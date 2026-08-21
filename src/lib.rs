//! Lush's core library. This is where the new lexer/parser/AST/executor
//! pipeline lives as it's built out, kept separate from `main.rs` so it's
//! directly unit-testable with `cargo test`.
//!
//! Status: Phase 2, Checkpoint 6. Adds `2>`, `2>>`, `2>&1`, and `&>`
//! redirect syntax on top of the existing `<`/`>`/`>>`. `2>` is only
//! recognized when the in-progress word is exactly a bare, unquoted "2"
//! immediately before `>` (no space), matching bash: `echo 2 > file`
//! prints the digit, `echo 2>file` redirects stderr. `2>&1` doesn't get
//! special lexer treatment at all, "2>" lexes as its own token and "&1"
//! just falls through as an ordinary word, it's the *parser* that
//! recognizes a redirect target of exactly "&1" as "duplicate onto
//! stdout" rather than a filename.

use std::fs;
use std::path::Path;

/// A single lexical token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// A word: a command name, argument, or filename.
    Word(Word),
    /// `|`
    Pipe,
    /// `&&`
    And,
    /// `||`
    Or,
    /// `;`
    Semicolon,
    /// `<`
    RedirectIn,
    /// `>`
    RedirectOut,
    /// `>>`
    RedirectAppend,
    /// `2>` (fd-prefixed, only fd 2/stderr is recognized)
    RedirectErrOut,
    /// `2>>`
    RedirectErrAppend,
    /// `&>` (both stdout and stderr to the same target)
    RedirectBoth,
    /// Marks the end of input. Every token stream ends with exactly one
    /// of these, so the parser never has to guess whether it's run off
    /// the end of the slice.
    Eof,
}

/// One piece of a word, tagged with how much special processing applies
/// to it. A word splits into multiple parts wherever the treatment
/// changes mid-word (`'abc'def` splits; plain `abcdef` doesn't).
///
/// Three kinds exist because variable expansion and filename globbing
/// disagree about double quotes: `$VAR` expands inside them but `*`
/// does NOT act as a wildcard there. Merging double-quoted runs into the
/// same part as unquoted text would erase exactly that distinction, so
/// they're kept apart even though both store plain text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WordPart {
    /// Came from inside single quotes. Never variable-expanded, never
    /// glob-active.
    Literal(String),
    /// Unquoted text. Variable-expanded AND glob-active (characters a
    /// substitution produces count here too, matching bash).
    Expandable(String),
    /// Came from inside double quotes. Variable-expanded, but its
    /// characters are glob-INERT (`echo "*"` lists a literal asterisk).
    DoubleQuoted(String),
}

impl WordPart {
    fn as_str(&self) -> &str {
        match self {
            WordPart::Literal(s) => s,
            WordPart::Expandable(s) => s,
            WordPart::DoubleQuoted(s) => s,
        }
    }
}

/// A word: a command name, argument, or redirect target, as a sequence of
/// quote-tagged parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub parts: Vec<WordPart>,
}

impl Word {
    /// Flattens all parts, literal and expandable alike, into one plain
    /// string, ignoring quote-kind entirely. This is what any shell logic
    /// that just wants "the text of this word" should use (command names,
    /// alias lookup, is_builtin checks, implicit-cd path checks, etc.);
    /// the parts distinction only matters to the variable-expansion pass.
    pub fn text(&self) -> String {
        self.parts.iter().map(WordPart::as_str).collect()
    }
}

/// Turns raw input into a flat token stream in a single pass. Whitespace
/// separates words but is not itself a token. Both `'` and `"` open a
/// quoted run; spaces and operator characters inside a quote are just
/// literal text until the matching close quote. An unmatched quote
/// swallows the rest of the input into one word rather than erroring,
/// same as bash's own behavior in that situation. A bare newline acts as
/// a statement separator, identical to `;` (so `echo a<newline>echo b`
/// runs as two commands, not one echo with combined args, and so an
/// embedded newline from e.g. a multi-line paste can't silently merge
/// into whatever word it lands next to).
pub fn lex(input: &str) -> Vec<Token> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut current = String::new();
    // Which treatment the run currently being built in `current` falls
    // under. All three kinds store plain text; they differ only in what
    // later passes may do to them (see WordPart), and a new part starts
    // whenever the kind changes mid-word.
    let mut run_kind = RunKind::Unquoted;
    // Whether the in-progress run was opened by an actual quote character
    // (as opposed to just being ordinary word text). This is what lets an
    // explicitly-quoted EMPTY string (`""`, `''`) survive as a zero-length
    // part — bash keeps `echo a "" b`'s empty argument, and lush needs the
    // same distinction to know which empty expansions to drop.
    let mut explicit_quote = false;
    let mut word_parts: Vec<WordPart> = Vec::new();
    let mut quote_char: Option<char> = None;
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        if let Some(qc) = quote_char {
            if c == qc {
                quote_char = None;
            } else {
                let kind = RunKind::from_quote(qc);
                if kind != run_kind {
                    flush_run(
                        &mut current,
                        run_kind,
                        &mut explicit_quote,
                        &mut word_parts,
                    );
                    run_kind = kind;
                }
                current.push(c);
            }
            i += 1;
            continue;
        }

        match c {
            '"' | '\'' => {
                // Opening a quote switches the kind immediately (not lazily
                // on the first quoted char) so that `""` — which accumulates
                // nothing — still leaves the run tagged as quoted.
                let kind = RunKind::from_quote(c);
                if kind != run_kind || !current.is_empty() {
                    flush_run(
                        &mut current,
                        run_kind,
                        &mut explicit_quote,
                        &mut word_parts,
                    );
                }
                run_kind = kind;
                explicit_quote = true;
                quote_char = Some(c);
                i += 1;
            }
            ' ' | '\t' => {
                flush_word(&mut current, &mut run_kind, &mut explicit_quote, &mut word_parts, &mut tokens);
                i += 1;
            }
            ';' | '\n' => {
                flush_word(&mut current, &mut run_kind, &mut explicit_quote, &mut word_parts, &mut tokens);
                tokens.push(Token::Semicolon);
                i += 1;
            }
            '&' if chars.get(i + 1) == Some(&'&') => {
                flush_word(&mut current, &mut run_kind, &mut explicit_quote, &mut word_parts, &mut tokens);
                tokens.push(Token::And);
                i += 2;
            }
            '&' if chars.get(i + 1) == Some(&'>') => {
                flush_word(&mut current, &mut run_kind, &mut explicit_quote, &mut word_parts, &mut tokens);
                tokens.push(Token::RedirectBoth);
                i += 2;
            }
            '|' if chars.get(i + 1) == Some(&'|') => {
                flush_word(&mut current, &mut run_kind, &mut explicit_quote, &mut word_parts, &mut tokens);
                tokens.push(Token::Or);
                i += 2;
            }
            '|' => {
                flush_word(&mut current, &mut run_kind, &mut explicit_quote, &mut word_parts, &mut tokens);
                tokens.push(Token::Pipe);
                i += 1;
            }
            // A bare, unquoted "2" immediately before '>'/'>>' (no space)
            // is bash's fd-prefix syntax for redirecting stderr, e.g.
            // `program 2>errors.txt`. Only recognized when the in-progress
            // word is EXACTLY "2" with nothing else accumulated yet
            // (word_parts empty) and it wasn't quoted, quoting it
            // ('2'>file) or gluing it to other text (12>file) makes it an
            // ordinary word instead, matching real shell behavior. Only fd
            // 2 is recognized, this shell doesn't support redirecting
            // arbitrary other file descriptors.
            '>' if chars.get(i + 1) == Some(&'>')
                && is_stderr_fd_prefix(&current, run_kind, &word_parts) =>
            {
                current.clear();
                tokens.push(Token::RedirectErrAppend);
                i += 2;
            }
            '>' if is_stderr_fd_prefix(&current, run_kind, &word_parts) => {
                current.clear();
                tokens.push(Token::RedirectErrOut);
                i += 1;
            }
            '>' if chars.get(i + 1) == Some(&'>') => {
                flush_word(&mut current, &mut run_kind, &mut explicit_quote, &mut word_parts, &mut tokens);
                tokens.push(Token::RedirectAppend);
                i += 2;
            }
            '>' => {
                flush_word(&mut current, &mut run_kind, &mut explicit_quote, &mut word_parts, &mut tokens);
                tokens.push(Token::RedirectOut);
                i += 1;
            }
            '<' => {
                flush_word(&mut current, &mut run_kind, &mut explicit_quote, &mut word_parts, &mut tokens);
                tokens.push(Token::RedirectIn);
                i += 1;
            }
            // A lone '&' (not doubled, not followed by '>') falls through
            // to here and becomes literal word text. This is also how
            // `2>&1`'s target gets lexed: after the RedirectErrOut token
            // consumes "2>", the following "&1" has nothing special about
            // it at the lexer level, it's just an ordinary word, and the
            // PARSER is what recognizes "&1" specifically as a
            // duplicate-onto-stdout marker rather than a filename.
            // Background jobs (Priority 15) aren't implemented yet, so a
            // genuinely standalone '&' also preserves today's behavior
            // rather than erroring on something the shell can't act on.
            _ => {
                if run_kind != RunKind::Unquoted {
                    flush_run(
                        &mut current,
                        run_kind,
                        &mut explicit_quote,
                        &mut word_parts,
                    );
                    run_kind = RunKind::Unquoted;
                }
                current.push(c);
                i += 1;
            }
        }
    }

    flush_word(&mut current, &mut run_kind, &mut explicit_quote, &mut word_parts, &mut tokens);
    tokens.push(Token::Eof);
    tokens
}

/// Which of the three WordPart treatments the in-progress text run gets.
/// The kinds never merge with each other even when they'd store the same
/// text — the part boundaries ARE the quote boundaries, which is exactly
/// what lets later passes (variable expansion vs globbing) treat each
/// segment by its own rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunKind {
    /// Outside any quotes.
    Unquoted,
    /// Inside single quotes.
    Single,
    /// Inside double quotes.
    Double,
}

impl RunKind {
    fn from_quote(qc: char) -> RunKind {
        if qc == '\'' {
            RunKind::Single
        } else {
            RunKind::Double
        }
    }

    fn into_part(self, text: String) -> WordPart {
        match self {
            RunKind::Single => WordPart::Literal(text),
            RunKind::Double => WordPart::DoubleQuoted(text),
            RunKind::Unquoted => WordPart::Expandable(text),
        }
    }

    fn is_quoted(self) -> bool {
        self != RunKind::Unquoted
    }
}

/// Whether the in-progress word is exactly a bare, unquoted "2", bash's
/// fd-prefix syntax for stderr redirection. Only true when nothing else
/// has been accumulated for this word yet (no prior parts) and the "2"
/// didn't come from inside any quotes, so `2>file` triggers it but
/// `'2'>file` or `12>file` don't.
fn is_stderr_fd_prefix(current: &str, run_kind: RunKind, word_parts: &[WordPart]) -> bool {
    !run_kind.is_quoted() && word_parts.is_empty() && current == "2"
}

/// Pushes the in-progress run onto `parts` as the WordPart matching its
/// `kind`, then clears `current`. Normally only non-empty runs are pushed;
/// the exception is an explicitly-quoted empty string — `""` or `''` with
/// nothing between the quotes — which pushes a zero-length part so the
/// shell can later tell "deliberately empty argument" apart from "variable
/// expanded to nothing" (bash keeps the first, drops the second). Called
/// whenever the run's kind is about to change, or the word itself ends.
fn flush_run(
    current: &mut String,
    kind: RunKind,
    explicit_quote: &mut bool,
    parts: &mut Vec<WordPart>,
) {
    if !current.is_empty() || (*explicit_quote && kind.is_quoted()) {
        let text = std::mem::take(current);
        parts.push(kind.into_part(text));
    }
    *explicit_quote = false;
}

/// Flushes the in-progress run, then pushes the accumulated parts as a
/// single Word token (if non-empty). Resets `run_kind` to Unquoted
/// afterward — every new word starts out unquoted until proven otherwise.
fn flush_word(
    current: &mut String,
    run_kind: &mut RunKind,
    explicit_quote: &mut bool,
    parts: &mut Vec<WordPart>,
    tokens: &mut Vec<Token>,
) {
    flush_run(current, *run_kind, explicit_quote, parts);
    if !parts.is_empty() {
        tokens.push(Token::Word(Word {
            parts: std::mem::take(parts),
        }));
    }
    *run_kind = RunKind::Unquoted;
}

// ---------------------------------------------------------------------
// Parser / AST
//
// Turns a token stream into a structured AST.
//
// Grammar (pipe binds tighter than &&/||, which bind tighter than ;,
// matching how the current shell already behaves):
//
//   sequence := and_or (';' and_or)*
//   and_or   := pipeline (('&&' | '||') pipeline)*
//   pipeline := command ('|' command)*
//   command  := (word | redirect)+
// ---------------------------------------------------------------------

/// A parsed command chain.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Command(SimpleCommand),
    Pipeline(Vec<SimpleCommand>),
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Sequence(Box<Node>, Box<Node>),
}

/// One command stage: its words (command name + args) and any
/// redirections attached to it.
#[derive(Debug, Clone, PartialEq)]
pub struct SimpleCommand {
    pub words: Vec<Word>,
    pub redirects: Vec<Redirect>,
}

/// A redirection and its target. The target is a full `Word` (not a
/// plain string) so redirect targets get the same quote-tracking as any
/// other word, `cat < $HOME/file.txt` will expand correctly once
/// variable expansion lands, no separate rework needed for redirects.
#[derive(Debug, Clone, PartialEq)]
pub enum Redirect {
    In(Word),
    Out(Word),
    Append(Word),
    /// `2> file`
    ErrOut(Word),
    /// `2>> file`
    ErrAppend(Word),
    /// `2>&1`: stderr duplicated onto wherever stdout currently points,
    /// rather than a file. Only `&1` is recognized as a duplication
    /// target (matching that this shell only tracks fd 1 and fd 2
    /// meaningfully); any other `&N` is just treated as an ordinary
    /// (if not very useful) filename.
    ErrToStdout,
    /// `&> file`: both stdout and stderr to the same target.
    Both(Word),
}

/// Parses a full line into an AST. Returns `Ok(None)` for empty input
/// (nothing to run), `Err` with a message for anything malformed (an
/// empty pipeline stage, a redirect with no target, or leftover tokens
/// the grammar didn't consume).
pub fn parse(input: &str) -> Result<Option<Node>, String> {
    let tokens = lex(input);
    let mut parser = Parser { tokens, pos: 0 };

    if matches!(parser.peek(), Token::Eof) {
        return Ok(None);
    }

    let node = parser.parse_sequence()?;

    if !matches!(parser.peek(), Token::Eof) {
        return Err(format!("unexpected token: {:?}", parser.peek()));
    }

    Ok(Some(node))
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let t = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn parse_sequence(&mut self) -> Result<Node, String> {
        let mut node = self.parse_and_or()?;
        while matches!(self.peek(), Token::Semicolon) {
            self.advance();
            if matches!(self.peek(), Token::Eof) {
                break; // trailing ';' with nothing after, treat as end
            }
            let rhs = self.parse_and_or()?;
            node = Node::Sequence(Box::new(node), Box::new(rhs));
        }
        Ok(node)
    }

    fn parse_and_or(&mut self) -> Result<Node, String> {
        let mut node = self.parse_pipeline()?;
        loop {
            match self.peek() {
                Token::And => {
                    self.advance();
                    let rhs = self.parse_pipeline()?;
                    node = Node::And(Box::new(node), Box::new(rhs));
                }
                Token::Or => {
                    self.advance();
                    let rhs = self.parse_pipeline()?;
                    node = Node::Or(Box::new(node), Box::new(rhs));
                }
                _ => break,
            }
        }
        Ok(node)
    }

    fn parse_pipeline(&mut self) -> Result<Node, String> {
        let mut commands = vec![self.parse_command()?];
        while matches!(self.peek(), Token::Pipe) {
            self.advance();
            commands.push(self.parse_command()?);
        }
        if commands.len() == 1 {
            Ok(Node::Command(commands.into_iter().next().unwrap()))
        } else {
            Ok(Node::Pipeline(commands))
        }
    }

    fn parse_command(&mut self) -> Result<SimpleCommand, String> {
        let mut words = Vec::new();
        let mut redirects = Vec::new();

        loop {
            match self.peek().clone() {
                Token::Word(w) => {
                    self.advance();
                    words.push(w);
                }
                Token::RedirectIn => {
                    self.advance();
                    redirects.push(Redirect::In(self.expect_word("<")?));
                }
                Token::RedirectOut => {
                    self.advance();
                    redirects.push(Redirect::Out(self.expect_word(">")?));
                }
                Token::RedirectAppend => {
                    self.advance();
                    redirects.push(Redirect::Append(self.expect_word(">>")?));
                }
                Token::RedirectErrOut => {
                    self.advance();
                    let target = self.expect_word("2>")?;
                    redirects.push(if is_fd1_ref(&target) {
                        Redirect::ErrToStdout
                    } else {
                        Redirect::ErrOut(target)
                    });
                }
                Token::RedirectErrAppend => {
                    self.advance();
                    let target = self.expect_word("2>>")?;
                    // "2>>&1" isn't really meaningful, append-vs-truncate
                    // doesn't apply to fd duplication, but if someone
                    // writes it, treat it the same as 2>&1 rather than
                    // trying to open a file literally named "&1".
                    redirects.push(if is_fd1_ref(&target) {
                        Redirect::ErrToStdout
                    } else {
                        Redirect::ErrAppend(target)
                    });
                }
                Token::RedirectBoth => {
                    self.advance();
                    redirects.push(Redirect::Both(self.expect_word("&>")?));
                }
                _ => break,
            }
        }

        if words.is_empty() && redirects.is_empty() {
            return Err("syntax error: expected a command".to_string());
        }

        Ok(SimpleCommand { words, redirects })
    }

    fn expect_word(&mut self, op: &str) -> Result<Word, String> {
        match self.advance() {
            Token::Word(w) => Ok(w),
            other => Err(format!(
                "syntax error: expected a filename after '{}', found {:?}",
                op, other
            )),
        }
    }
}

/// Whether a redirect target word is exactly "&1", bash's syntax for
/// "duplicate onto stdout" rather than a filename. Only recognizes fd 1
/// specifically, matching that this shell only tracks fd 1 and fd 2
/// meaningfully; a word like "&2" or "&foo" is just an ordinary filename.
fn is_fd1_ref(word: &Word) -> bool {
    word.text() == "&1"
}

#[cfg(test)]
mod parser_tests {
    use super::*;

    fn word(s: &str) -> Word {
        Word {
            parts: vec![WordPart::Expandable(s.to_string())],
        }
    }

    fn cmd(words: &[&str]) -> SimpleCommand {
        SimpleCommand {
            words: words.iter().map(|s| word(s)).collect(),
            redirects: vec![],
        }
    }

    #[test]
    fn single_command() {
        assert_eq!(
            parse("echo hello").unwrap(),
            Some(Node::Command(cmd(&["echo", "hello"])))
        );
    }

    #[test]
    fn empty_input_parses_to_none() {
        assert_eq!(parse("").unwrap(), None);
    }

    #[test]
    fn pipeline_of_two() {
        assert_eq!(
            parse("cat file.txt | grep foo").unwrap(),
            Some(Node::Pipeline(vec![
                cmd(&["cat", "file.txt"]),
                cmd(&["grep", "foo"]),
            ]))
        );
    }

    #[test]
    fn redirect_attaches_to_its_command() {
        assert_eq!(
            parse("echo hi > out.txt").unwrap(),
            Some(Node::Command(SimpleCommand {
                words: vec![word("echo"), word("hi")],
                redirects: vec![Redirect::Out(word("out.txt"))],
            }))
        );
    }

    #[test]
    fn redirect_glued_no_whitespace() {
        // Same case the roadmap flagged as broken: the parser builds the
        // identical AST whether or not there's a space, since the lexer
        // already normalized this at the token level.
        assert_eq!(parse("echo hi>out.txt").unwrap(), parse("echo hi > out.txt").unwrap());
    }

    #[test]
    fn and_or_left_associative() {
        // `a && b || c` should be (a && b) || c, not a && (b || c).
        assert_eq!(
            parse("true && echo a || echo b").unwrap(),
            Some(Node::Or(
                Box::new(Node::And(
                    Box::new(Node::Command(cmd(&["true"]))),
                    Box::new(Node::Command(cmd(&["echo", "a"]))),
                )),
                Box::new(Node::Command(cmd(&["echo", "b"]))),
            ))
        );
    }

    #[test]
    fn semicolon_sequence() {
        assert_eq!(
            parse("echo a ; echo b").unwrap(),
            Some(Node::Sequence(
                Box::new(Node::Command(cmd(&["echo", "a"]))),
                Box::new(Node::Command(cmd(&["echo", "b"]))),
            ))
        );
    }

    #[test]
    fn pipe_binds_tighter_than_and() {
        // foo && bar | grep hello  →  And(foo, Pipeline([bar, grep hello]))
        assert_eq!(
            parse("foo && bar | grep hello").unwrap(),
            Some(Node::And(
                Box::new(Node::Command(cmd(&["foo"]))),
                Box::new(Node::Pipeline(vec![cmd(&["bar"]), cmd(&["grep", "hello"])])),
            ))
        );
    }

    #[test]
    fn quoted_pipe_is_not_a_pipeline() {
        // Double-quoted text stays one DoubleQuoted part; the pipe inside
        // never becomes an operator.
        assert_eq!(
            parse(r#"echo "hello | world""#).unwrap(),
            Some(Node::Command(SimpleCommand {
                words: vec![
                    word("echo"),
                    Word {
                        parts: vec![WordPart::DoubleQuoted("hello | world".to_string())]
                    },
                ],
                redirects: vec![],
            }))
        );
    }

    #[test]
    fn empty_pipeline_stage_is_an_error() {
        assert!(parse("echo hi | | wc").is_err());
    }

    #[test]
    fn dangling_redirect_is_an_error() {
        assert!(parse("echo hi >").is_err());
    }

    #[test]
    fn trailing_operator_is_an_error() {
        assert!(parse("echo hi &&").is_err());
    }

    #[test]
    fn stderr_redirect_to_file() {
        assert_eq!(
            parse("prog 2> err.txt").unwrap(),
            Some(Node::Command(SimpleCommand {
                words: vec![word("prog")],
                redirects: vec![Redirect::ErrOut(word("err.txt"))],
            }))
        );
    }

    #[test]
    fn stderr_append_to_file() {
        assert_eq!(
            parse("prog 2>> err.txt").unwrap(),
            Some(Node::Command(SimpleCommand {
                words: vec![word("prog")],
                redirects: vec![Redirect::ErrAppend(word("err.txt"))],
            }))
        );
    }

    #[test]
    fn stderr_duplicated_onto_stdout() {
        assert_eq!(
            parse("prog 2>&1").unwrap(),
            Some(Node::Command(SimpleCommand {
                words: vec![word("prog")],
                redirects: vec![Redirect::ErrToStdout],
            }))
        );
    }

    #[test]
    fn both_streams_to_one_file() {
        assert_eq!(
            parse("prog &> all.txt").unwrap(),
            Some(Node::Command(SimpleCommand {
                words: vec![word("prog")],
                redirects: vec![Redirect::Both(word("all.txt"))],
            }))
        );
    }

    #[test]
    fn stdout_file_then_stderr_duplicated_onto_it() {
        // The common idiom: program > out.txt 2>&1
        assert_eq!(
            parse("prog > out.txt 2>&1").unwrap(),
            Some(Node::Command(SimpleCommand {
                words: vec![word("prog")],
                redirects: vec![Redirect::Out(word("out.txt")), Redirect::ErrToStdout],
            }))
        );
    }

    #[test]
    fn ampersand_two_is_just_a_filename() {
        // Only &1 is recognized as fd duplication, &2 (or anything else)
        // is just an ordinary (if unhelpful) filename.
        assert_eq!(
            parse("prog 2> &2").unwrap(),
            Some(Node::Command(SimpleCommand {
                words: vec![word("prog")],
                redirects: vec![Redirect::ErrOut(word("&2"))],
            }))
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An unquoted-or-double-quoted word token: one Expandable part.
    fn w(s: &str) -> Token {
        Token::Word(Word {
            parts: vec![WordPart::Expandable(s.to_string())],
        })
    }

    /// A single-quoted word token: one Literal part.
    fn lit(s: &str) -> Token {
        Token::Word(Word {
            parts: vec![WordPart::Literal(s.to_string())],
        })
    }

    #[test]
    fn simple_command() {
        let tokens = lex("echo hello world");
        assert_eq!(tokens, vec![w("echo"), w("hello"), w("world"), Token::Eof]);
    }

    #[test]
    fn pipe_inside_quotes_is_not_a_pipe() {
        // Regression test for the exact bug called out in the roadmap: a
        // pipe character inside quotes must stay part of the word, not
        // become a Pipe token. Double-quoted text is a DoubleQuoted part.
        let tokens = lex(r#"echo "hello | world""#);
        assert_eq!(
            tokens,
            vec![
                w("echo"),
                Token::Word(Word {
                    parts: vec![WordPart::DoubleQuoted("hello | world".to_string())]
                }),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn redirect_without_whitespace() {
        let tokens = lex("echo hello>out.txt");
        assert_eq!(
            tokens,
            vec![w("echo"), w("hello"), Token::RedirectOut, w("out.txt"), Token::Eof]
        );
    }

    #[test]
    fn redirect_with_whitespace_still_works() {
        let tokens = lex("echo hello > out.txt");
        assert_eq!(
            tokens,
            vec![w("echo"), w("hello"), Token::RedirectOut, w("out.txt"), Token::Eof]
        );
    }

    #[test]
    fn append_redirect_glued() {
        let tokens = lex("echo hi>>out.txt");
        assert_eq!(
            tokens,
            vec![w("echo"), w("hi"), Token::RedirectAppend, w("out.txt"), Token::Eof]
        );
    }

    #[test]
    fn input_redirect_glued() {
        let tokens = lex("sort <in.txt");
        assert_eq!(tokens, vec![w("sort"), Token::RedirectIn, w("in.txt"), Token::Eof]);
    }

    #[test]
    fn pipeline() {
        let tokens = lex("cat file.txt | grep foo | wc -l");
        assert_eq!(
            tokens,
            vec![
                w("cat"),
                w("file.txt"),
                Token::Pipe,
                w("grep"),
                w("foo"),
                Token::Pipe,
                w("wc"),
                w("-l"),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn and_or_semicolon() {
        let tokens = lex("true && echo a || echo b ; echo c");
        assert_eq!(
            tokens,
            vec![
                w("true"),
                Token::And,
                w("echo"),
                w("a"),
                Token::Or,
                w("echo"),
                w("b"),
                Token::Semicolon,
                w("echo"),
                w("c"),
                Token::Eof,
            ]
        );
    }

    /// A double-quoted word token: one DoubleQuoted part.
    fn dq(s: &str) -> Token {
        Token::Word(Word {
            parts: vec![WordPart::DoubleQuoted(s.to_string())],
        })
    }

    #[test]
    fn single_and_double_quotes() {
        // Three kinds, three parts: single-quoted is Literal (never
        // expanded), double-quoted is DoubleQuoted (expanded but
        // glob-inert), unquoted is Expandable.
        let tokens = lex(r#"echo 'single' "double""#);
        assert_eq!(tokens, vec![w("echo"), lit("single"), dq("double"), Token::Eof]);
    }

    #[test]
    fn adjacent_quoted_and_unquoted_merge_into_one_word() {
        // Matches real shell semantics: "ab" + quoted "c d" + "ef" glued
        // with no space between them is ONE word, "abc def". The parts
        // stay separate (unquoted / double-quoted / unquoted) because
        // globbing needs the distinction, but the word itself doesn't
        // split and the flattened text is identical.
        let tokens = lex(r#"ab"c d"ef"#);
        assert_eq!(tokens.len(), 2); // one Word + Eof
        let Token::Word(merged) = &tokens[0] else {
            panic!("expected a Word token");
        };
        assert_eq!(merged.text(), "abc def");
        assert_eq!(
            tokens,
            vec![
                Token::Word(Word {
                    parts: vec![
                        WordPart::Expandable("ab".to_string()),
                        WordPart::DoubleQuoted("c d".to_string()),
                        WordPart::Expandable("ef".to_string()),
                    ]
                }),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn mixed_quote_word_splits_into_precise_segments() {
        // The actual point of choosing precise (over coarse) tracking:
        // 'abc' is single-quoted (Literal), def is unquoted (Expandable),
        // glued with no space. These must stay as two distinct parts so
        // a later expansion pass only touches the Expandable one.
        let tokens = lex("'abc'def");
        assert_eq!(
            tokens,
            vec![
                Token::Word(Word {
                    parts: vec![
                        WordPart::Literal("abc".to_string()),
                        WordPart::Expandable("def".to_string()),
                    ]
                }),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn expandable_then_literal_segment() {
        // Same idea, reversed order, and with $ content specifically
        // (even though $ isn't given any special meaning by the lexer
        // yet, that's the next checkpoint): $HOME stays Expandable,
        // '/literal' stays Literal, as two separate parts.
        let tokens = lex("$HOME'/literal'");
        assert_eq!(
            tokens,
            vec![
                Token::Word(Word {
                    parts: vec![
                        WordPart::Expandable("$HOME".to_string()),
                        WordPart::Literal("/literal".to_string()),
                    ]
                }),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn empty_input_is_just_eof() {
        assert_eq!(lex(""), vec![Token::Eof]);
    }

    #[test]
    fn lone_ampersand_is_literal_for_now() {
        // Background jobs (&) aren't implemented yet (Priority 15), so a
        // single & stays literal rather than erroring, preserving
        // today's behavior until job control lands.
        let tokens = lex("echo a & b");
        assert_eq!(tokens, vec![w("echo"), w("a"), w("&"), w("b"), Token::Eof]);
    }

    #[test]
    fn empty_quotes_produce_zero_length_parts() {
        // Explicitly-quoted emptiness must survive as a real (empty)
        // argument — that's what lets the executor keep `echo a "" b`'s
        // middle word while dropping a variable that expanded to nothing.
        let tokens = lex(r#""""#);
        assert_eq!(
            tokens,
            vec![
                Token::Word(Word {
                    parts: vec![WordPart::DoubleQuoted(String::new())]
                }),
                Token::Eof,
            ]
        );
        let tokens = lex("''");
        assert_eq!(
            tokens,
            vec![
                Token::Word(Word {
                    parts: vec![WordPart::Literal(String::new())]
                }),
                Token::Eof,
            ]
        );
        // Glued between text, the empty quote adds a part but not a word.
        let tokens = lex(r#"a""b"#);
        assert_eq!(tokens.len(), 2);
    }

    #[test]
    fn embedded_newline_terminates_a_statement() {
        // Regression test: a literal newline inside the input (e.g. from
        // a multi-line paste arriving as one string) must act as a
        // separator, same as ';', not get silently absorbed into
        // whichever word it happens to land next to.
        let tokens = lex("echo work\ntrue");
        assert_eq!(
            tokens,
            vec![w("echo"), w("work"), Token::Semicolon, w("true"), Token::Eof]
        );
    }

    #[test]
    fn glued_2_redirects_stderr() {
        // No space: "2>" is one operator token.
        let tokens = lex("prog 2>err.txt");
        assert_eq!(
            tokens,
            vec![w("prog"), Token::RedirectErrOut, w("err.txt"), Token::Eof]
        );
    }

    #[test]
    fn spaced_2_is_just_the_number_two() {
        // The critical disambiguation this whole feature rests on: with
        // a space, "2" is an ordinary word (echo would print the digit),
        // and ">" is the normal stdout redirect, completely different
        // from the glued form above.
        let tokens = lex("echo 2 > file");
        assert_eq!(
            tokens,
            vec![w("echo"), w("2"), Token::RedirectOut, w("file"), Token::Eof]
        );
    }

    #[test]
    fn glued_2_append_redirects_stderr() {
        let tokens = lex("prog 2>>err.txt");
        assert_eq!(
            tokens,
            vec![w("prog"), Token::RedirectErrAppend, w("err.txt"), Token::Eof]
        );
    }

    #[test]
    fn quoted_two_is_not_an_fd_prefix() {
        // Quoting the "2" suppresses the special meaning, same principle
        // as any other quoted text staying literal.
        let tokens = lex("prog '2'>file");
        assert_eq!(
            tokens,
            vec![w("prog"), lit("2"), Token::RedirectOut, w("file"), Token::Eof]
        );
    }

    #[test]
    fn digit_glued_to_other_text_is_not_an_fd_prefix() {
        // "12" isn't "2", the fd-prefix rule only fires when the
        // in-progress word is exactly the single character "2".
        let tokens = lex("prog 12>file");
        assert_eq!(
            tokens,
            vec![w("prog"), w("12"), Token::RedirectOut, w("file"), Token::Eof]
        );
    }

    #[test]
    fn ampersand_greater_than_is_redirect_both() {
        let tokens = lex("prog &>all.txt");
        assert_eq!(
            tokens,
            vec![w("prog"), Token::RedirectBoth, w("all.txt"), Token::Eof]
        );
    }

    #[test]
    fn ampersand_ampersand_still_wins_over_ampersand_greater_than() {
        // Make sure adding the &> check didn't disturb && recognition.
        let tokens = lex("true && echo hi");
        assert_eq!(tokens, vec![w("true"), Token::And, w("echo"), w("hi"), Token::Eof]);
    }
}

// ---------------------------------------------------------------------
// Variable expansion
//
// Resolves $VAR / ${VAR} references inside a Word's Expandable parts.
// Literal parts (single-quoted) are never touched. Deliberately kept
// dependency-free, no environment access here at all, `lookup` is
// supplied by the caller (main.rs), which is what keeps this testable
// without any process state and keeps parsing separate from execution.
// ---------------------------------------------------------------------

/// Expands a Word into its final string, using `lookup` to resolve any
/// `$VAR` / `${VAR}` references found in Expandable parts. Literal parts
/// are copied through untouched, that's the whole point of tracking them
/// separately. `lookup` returning `None` (variable not set) expands to
/// an empty string, matching real shell behavior rather than erroring.
pub fn expand_word(word: &Word, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    expand_word_tracked(word, lookup).0
}

/// Same expansion as `expand_word`, but also returns one flag per output
/// character marking whether it came from a quoted run (a Literal part).
/// Glob expansion consults these flags so `echo "*"` stays literal while
/// `echo *.txt` expands. Characters PRODUCED by a variable substitution
/// count as unquoted, so a variable holding a pattern still globs —
/// `foo='*.txt'; echo $foo` behaves like typing the pattern directly,
/// same as bash.
pub fn expand_word_tracked(
    word: &Word,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> (String, Vec<bool>) {
    let mut text = String::new();
    let mut quoted = Vec::new();
    for part in &word.parts {
        match part {
            WordPart::Literal(s) => {
                text.push_str(s);
                quoted.extend(std::iter::repeat(true).take(s.chars().count()));
            }
            WordPart::DoubleQuoted(s) => {
                // $VAR expands inside double quotes, but the characters —
                // both original and substituted — are glob-INERT.
                let (expanded, _) = expand_variables_tracked(s, lookup);
                text.push_str(&expanded);
                quoted.extend(std::iter::repeat(true).take(expanded.chars().count()));
            }
            WordPart::Expandable(s) => {
                let (expanded, flags) = expand_variables_tracked(s, lookup);
                text.push_str(&expanded);
                quoted.extend(flags);
            }
        }
    }
    (text, quoted)
}

/// Scans text for `$NAME` and `${NAME}` references and replaces each
/// with `lookup(NAME)` (or an empty string if unset). A `$` not followed
/// by `{` or a valid variable-name start (letter or underscore) is left
/// as a literal `$`, so things like a bare trailing `$` or `$5` (no
/// positional parameters yet) don't get mangled. Also reports one flag
/// per emitted character for glob-quote tracking (always false here —
/// everything this emits is Expandable text or substitution output,
/// both glob-active; quoted=true flags originate in expand_word_tracked's
/// Literal arm).
fn expand_variables_tracked(
    text: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> (String, Vec<bool>) {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::new();
    let mut flags = Vec::new();

    // Small helper keeping every emission's char and flag in lockstep.
    fn emit(out: &mut String, flags: &mut Vec<bool>, c: char) {
        out.push(c);
        flags.push(false);
    }

    let mut i = 0;

    while i < chars.len() {
        if chars[i] != '$' {
            emit(&mut result, &mut flags, chars[i]);
            i += 1;
            continue;
        }

        if chars.get(i + 1) == Some(&'{') {
            if let Some(close) = find_closing_brace(&chars, i + 2) {
                let name: String = chars[i + 2..close].iter().collect();
                for c in lookup(&name).unwrap_or_default().chars() {
                    emit(&mut result, &mut flags, c);
                }
                i = close + 1;
            } else {
                // No matching '}': treat this '$' as literal and let the
                // '{' be scanned normally on the next pass.
                emit(&mut result, &mut flags, '$');
                i += 1;
            }
            continue;
        }

        if is_var_start(chars.get(i + 1).copied()) {
            let start = i + 1;
            let mut end = start;
            while end < chars.len() && is_var_char(chars[end]) {
                end += 1;
            }
            let name: String = chars[start..end].iter().collect();
            for c in lookup(&name).unwrap_or_default().chars() {
                emit(&mut result, &mut flags, c);
            }
            i = end;
            continue;
        }

        // '$' not followed by '{' or a valid name start: literal.
        emit(&mut result, &mut flags, '$');
        i += 1;
    }

    (result, flags)
}

fn find_closing_brace(chars: &[char], start: usize) -> Option<usize> {
    chars[start..].iter().position(|&c| c == '}').map(|p| start + p)
}

fn is_var_start(c: Option<char>) -> bool {
    matches!(c, Some(c) if c.is_ascii_alphabetic() || c == '_')
}

fn is_var_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

// ---------------------------------------------------------------------
// Filename globbing
//
// Expands `*`, `?`, and `[...]` patterns in a word against the real
// filesystem, running after variable expansion (so $HOME/*.txt works).
// Only UNQUOTED characters act as pattern characters — the quote flags
// come from expand_word_tracked — so `echo "*"` stays literal even when
// files would match, while a variable holding a pattern still expands,
// both matching bash.
//
// Patterns may span directories (`src/*.rs`); every component except
// the last must resolve to a directory. A trailing `/` restricts
// matches to directories and is preserved in the output (`*/`). Leading
// dotfiles are protected: a name starting with `.` only matches when
// the pattern's own first character is a `.` (POSIX rule, so `*` skips
// `.hidden` but `.*` catches it).
//
// Zero matches yields an empty Vec; callers treat that as "keep the
// word literal", which is bash's default (nullglob off) — so `rm *.txt`
// with nothing to delete fails the way bash's rm would, not silently.
// ---------------------------------------------------------------------

/// Expands one word's glob pattern against the filesystem. Returns the
/// sorted list of matched paths (relative to cwd unless the pattern is
/// absolute), or an empty Vec when the word carries no active pattern
/// characters or nothing matches.
pub fn glob_expand_word(word: &Word, lookup: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let (text, quoted) = expand_word_tracked(word, lookup);
    let chars: Vec<char> = text.chars().collect();
    if !has_active_glob(&chars, &quoted) {
        return Vec::new();
    }

    // Split into '/'-separated components, keeping each component's
    // per-character quote flags aligned. Empty components (from `//`,
    // a leading '/', or a trailing '/') carry no information — absolute-
    // ness and dir-only-ness are tracked explicitly below.
    let absolute = chars.first() == Some(&'/');
    let trailing_slash = chars.last() == Some(&'/');
    let mut components: Vec<Vec<(char, bool)>> = Vec::new();
    let mut current: Vec<(char, bool)> = Vec::new();
    for (i, c) in chars.iter().copied().enumerate() {
        if c == '/' {
            components.push(std::mem::take(&mut current));
        } else {
            current.push((c, quoted[i]));
        }
    }
    components.push(current);
    components.retain(|c| !c.is_empty());
    if components.is_empty() {
        return Vec::new();
    }

    // Walk the component chain. `frontier` holds the directories matched
    // so far; intermediate components must be directories, the final one
    // contributes results.
    let mut frontier: Vec<String> = vec![if absolute { "/".to_string() } else { String::new() }];
    let mut results: Vec<String> = Vec::new();
    let last = components.len() - 1;

    for (idx, comp) in components.iter().enumerate() {
        if idx < last {
            let mut next: Vec<String> = Vec::new();
            for base in &frontier {
                for (name, is_dir) in list_dir_entries(base) {
                    if !dot_rule_ok(&name, comp) || !is_dir {
                        continue;
                    }
                    if glob_match_component(comp, &name.chars().collect::<Vec<_>>()) {
                        next.push(join_path(base, &name));
                    }
                }
            }
            frontier = next;
            if frontier.is_empty() {
                return Vec::new();
            }
        } else {
            for base in &frontier {
                for (name, is_dir) in list_dir_entries(base) {
                    if !dot_rule_ok(&name, comp) {
                        continue;
                    }
                    if !glob_match_component(comp, &name.chars().collect::<Vec<_>>()) {
                        continue;
                    }
                    if trailing_slash && !is_dir {
                        continue;
                    }
                    let full = join_path(base, &name);
                    results.push(if trailing_slash { format!("{}/", full) } else { full });
                }
            }
        }
    }

    results.sort();
    results
}

/// Dotfile protection: names beginning with `.` require the pattern's
/// first character to be a literal `.` (quoting it doesn't matter — it's
/// still explicit).
fn dot_rule_ok(name: &str, comp: &[(char, bool)]) -> bool {
    if !name.starts_with('.') {
        return true;
    }
    comp.first().map(|(c, _)| *c) == Some('.')
}

/// Whether the word contains any unquoted `*`, `?`, or `[` — i.e. any
/// character that could make it a pattern worth hitting the filesystem for.
fn has_active_glob(chars: &[char], quoted: &[bool]) -> bool {
    chars
        .iter()
        .zip(quoted)
        .any(|(&c, &q)| !q && matches!(c, '*' | '?' | '['))
}

/// Matches one path component against one pattern component. Classic
/// recursive glob: unquoted `*` consumes any run (including empty),
/// unquoted `?` consumes exactly one character, unquoted `[...]` is a
/// character class, everything else matches itself literally. Quoted
/// pattern characters always match themselves only.
fn glob_match_component(pat: &[(char, bool)], name: &[char]) -> bool {
    let Some(&(pc, pq)) = pat.first() else {
        return name.is_empty();
    };
    let rest = &pat[1..];

    if pq {
        return name.first() == Some(&pc) && glob_match_component(rest, &name[1..]);
    }

    match pc {
        '*' => (0..=name.len()).any(|split| glob_match_component(rest, &name[split..])),
        '?' => !name.is_empty() && glob_match_component(rest, &name[1..]),
        '[' => match parse_bracket_class(rest) {
            Some((negated, items, after)) => {
                let Some(&first) = name.first() else {
                    return false;
                };
                if class_contains(&items, first) != negated {
                    glob_match_component(after, &name[1..])
                } else {
                    false
                }
            }
            // Unterminated `[`: treat the bracket as an ordinary character,
            // same fallback bash uses.
            None => name.first() == Some(&pc) && glob_match_component(rest, &name[1..]),
        },
        _ => name.first() == Some(&pc) && glob_match_component(rest, &name[1..]),
    }
}

/// One class member: either a single char or an inclusive range.
type ClassItem = (char, char);

/// Parses the contents of a `[...]` class from `pat`, which starts just
/// after the opening `[`. Returns (negated, members, remainder-after-`]`),
/// or None when there's no closing `]`.
///
/// `!` or `^` right after the bracket negates the class. A `]` appearing
/// immediately after that (or first) is a literal member, so `[!]]` means
/// "anything but ]". Ranges are written `a-z`; `-` adjacent to the
/// closing bracket is a literal dash.
fn parse_bracket_class(
    pat: &[(char, bool)],
) -> Option<(bool, Vec<ClassItem>, &[(char, bool)])> {
    let mut idx = 0usize;
    let negated = matches!(pat.get(idx), Some(('!', _)) | Some(('^', _)));
    if negated {
        idx += 1;
    }
    let mut items: Vec<ClassItem> = Vec::new();

    // A `]` appearing immediately after `[` or `[!` is a literal member
    // rather than the terminator — that's what makes `[!]]` ("anything
    // but ]") expressible.
    if matches!(pat.get(idx), Some((']', _))) {
        items.push((']', ']'));
        idx += 1;
    }

    loop {
        let &(c, _) = pat.get(idx)?;
        if c == ']' {
            return Some((negated, items, &pat[idx + 1..]));
        }

        // Range? lo '-' hi where hi isn't the closing bracket.
        if let (Some((next, _)), Some((after_next, _))) = (pat.get(idx + 1), pat.get(idx + 2)) {
            if *next == '-' && *after_next != ']' {
                items.push((c, *after_next));
                idx += 3;
                continue;
            }
        }

        items.push((c, c));
        idx += 1;
    }
}

/// Whether `c` falls inside any member (single chars are lo==hi ranges).
fn class_contains(items: &[ClassItem], c: char) -> bool {
    items.iter().any(|&(lo, hi)| c >= lo && c <= hi)
}

/// Lists `(name, is_dir)` for a directory; empty string means cwd.
/// Symlinks are resolved via metadata, so a link pointing at a directory
/// counts as one (matches how bash treats `*/`).
fn list_dir_entries(dir: &str) -> Vec<(String, bool)> {
    let path = if dir.is_empty() { Path::new(".") } else { Path::new(dir) };
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.metadata().map(|m| m.is_dir()).unwrap_or(false);
            Some((name, is_dir))
        })
        .collect()
}

/// Joins a walked base path with a matched name. An empty base is cwd
/// (results stay relative, like bash); "/" is root (no double slash).
fn join_path(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_string()
    } else if base.ends_with('/') {
        format!("{}{}", base, name)
    } else {
        format!("{}/{}", base, name)
    }
}

#[cfg(test)]
mod expansion_tests {
    use super::*;

    fn lookup(name: &str) -> Option<String> {
        match name {
            "HOME" => Some("/home/lumi".to_string()),
            "USER" => Some("lumi".to_string()),
            _ => None,
        }
    }

    fn expandable(s: &str) -> Word {
        Word {
            parts: vec![WordPart::Expandable(s.to_string())],
        }
    }

    fn literal(s: &str) -> Word {
        Word {
            parts: vec![WordPart::Literal(s.to_string())],
        }
    }

    #[test]
    fn expands_simple_variable() {
        assert_eq!(expand_word(&expandable("$HOME"), &lookup), "/home/lumi");
    }

    #[test]
    fn expands_braced_variable() {
        assert_eq!(expand_word(&expandable("${HOME}"), &lookup), "/home/lumi");
    }

    #[test]
    fn expands_variable_with_surrounding_text() {
        // The exact case from the roadmap: cd $HOME/Documents.
        assert_eq!(expand_word(&expandable("$HOME/Documents"), &lookup), "/home/lumi/Documents");
    }

    #[test]
    fn braced_form_allows_adjacent_text_with_no_ambiguity() {
        // ${USER}_backup: without braces this would try to look up a
        // variable literally named "USER_backup". With braces the name
        // boundary is explicit.
        assert_eq!(expand_word(&expandable("${USER}_backup"), &lookup), "lumi_backup");
    }

    #[test]
    fn undefined_variable_expands_to_empty_string() {
        assert_eq!(expand_word(&expandable("$NOPE"), &lookup), "");
    }

    #[test]
    fn literal_part_is_never_expanded() {
        // The whole point of single quotes: echo '$HOME' must print
        // literally, not expand.
        assert_eq!(expand_word(&literal("$HOME"), &lookup), "$HOME");
    }

    #[test]
    fn mixed_literal_and_expandable_parts_expand_independently() {
        // '$HOME'-$USER glued together: the single-quoted part stays
        // literal, the unquoted part still expands. This is the actual
        // payoff of choosing precise (over coarse) quote tracking.
        let word = Word {
            parts: vec![
                WordPart::Literal("$HOME".to_string()),
                WordPart::Expandable("-$USER".to_string()),
            ],
        };
        assert_eq!(expand_word(&word, &lookup), "$HOME-lumi");
    }

    #[test]
    fn lone_dollar_sign_stays_literal() {
        // $5 isn't a valid variable name start (digit), so it's left as
        // literal text rather than being mangled.
        assert_eq!(expand_word(&expandable("cost: $5"), &lookup), "cost: $5");
    }

    #[test]
    fn unclosed_brace_stays_literal() {
        assert_eq!(expand_word(&expandable("${HOME"), &lookup), "${HOME");
    }

    #[test]
    fn no_dollar_sign_is_unchanged() {
        assert_eq!(expand_word(&expandable("plain text"), &lookup), "plain text");
    }
}

// ---------------------------------------------------------------------
// Assignment detection
//
// Recognizes the standalone `NAME=value` form (a single word shaped like
// an assignment, with nothing else on the line) and pulls out the name
// and a Word representing the value, still unexpanded, so the caller can
// run it through expand_word() the same way any other word would be.
// Bash's other assignment form, `NAME=value some_command args...` (a
// temporary variable scoped to just that one command), is NOT
// recognized here, that's a distinct feature and out of scope for this
// checkpoint.
// ---------------------------------------------------------------------

/// Detects whether a command is actually a shell-variable assignment
/// rather than something to execute. Returns the variable name and its
/// value (as an unexpanded Word, preserving quote-kind, so `foo='$HOME'`
/// keeps that value Literal) if this looks like an assignment.
pub fn parse_assignment(words: &[Word]) -> Option<(String, Word)> {
    if words.len() != 1 {
        return None;
    }
    let first_part = words[0].parts.first()?;
    let text = match first_part {
        WordPart::Expandable(s) => s,
        // A quoted var name makes no sense, whichever quote style.
        WordPart::Literal(_) | WordPart::DoubleQuoted(_) => return None,
    };
    let eq_pos = text.find('=')?;
    let name = &text[..eq_pos];
    if !is_valid_var_name(name) {
        return None;
    }
    let remainder = &text[eq_pos + 1..];

    let mut value_parts = Vec::new();
    if !remainder.is_empty() {
        value_parts.push(WordPart::Expandable(remainder.to_string()));
    }
    value_parts.extend(words[0].parts[1..].iter().cloned());

    Some((name.to_string(), Word { parts: value_parts }))
}

/// Whether `name` is a valid shell variable name: starts with a letter
/// or underscore, followed by any number of letters, digits, or
/// underscores. Same rule real shells use.
fn is_valid_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod assignment_tests {
    use super::*;

    fn expandable(s: &str) -> Word {
        Word {
            parts: vec![WordPart::Expandable(s.to_string())],
        }
    }

    #[test]
    fn simple_assignment() {
        let words = vec![expandable("foo=bar")];
        let (name, value) = parse_assignment(&words).unwrap();
        assert_eq!(name, "foo");
        assert_eq!(value, expandable("bar"));
    }

    #[test]
    fn assignment_with_variable_reference_stays_unexpanded() {
        // The caller is responsible for expanding the value afterward,
        // parse_assignment itself never touches $ content.
        let words = vec![expandable("foo=$HOME")];
        let (name, value) = parse_assignment(&words).unwrap();
        assert_eq!(name, "foo");
        assert_eq!(value, expandable("$HOME"));
    }

    #[test]
    fn assignment_with_quoted_value_preserves_literal_part() {
        // foo='$HOME' lexes to two parts: Expandable("foo=") and
        // Literal("$HOME"). The value should end up as just the Literal
        // part, so it never expands later.
        let words = vec![Word {
            parts: vec![
                WordPart::Expandable("foo=".to_string()),
                WordPart::Literal("$HOME".to_string()),
            ],
        }];
        let (name, value) = parse_assignment(&words).unwrap();
        assert_eq!(name, "foo");
        assert_eq!(
            value,
            Word {
                parts: vec![WordPart::Literal("$HOME".to_string())]
            }
        );
    }

    #[test]
    fn empty_value_is_a_valid_assignment() {
        let words = vec![expandable("foo=")];
        let (name, value) = parse_assignment(&words).unwrap();
        assert_eq!(name, "foo");
        assert_eq!(value, Word { parts: vec![] });
    }

    #[test]
    fn multiple_words_is_not_an_assignment() {
        // foo=bar echo hi: bash's temporary per-command environment
        // form, deliberately not recognized here, out of scope.
        let words = vec![expandable("foo=bar"), expandable("echo"), expandable("hi")];
        assert_eq!(parse_assignment(&words), None);
    }

    #[test]
    fn invalid_identifier_is_not_an_assignment() {
        // Starts with a digit: not a valid variable name, so this is
        // just an ordinary (if doomed to fail) command name.
        let words = vec![expandable("2foo=bar")];
        assert_eq!(parse_assignment(&words), None);
    }

    #[test]
    fn no_equals_sign_is_not_an_assignment() {
        let words = vec![expandable("echo")];
        assert_eq!(parse_assignment(&words), None);
    }

    #[test]
    fn quoted_name_is_not_an_assignment() {
        // A word starting with a Literal part can't be a valid
        // assignment, quoting a variable *name* makes no sense.
        let words = vec![Word {
            parts: vec![WordPart::Literal("foo=bar".to_string())],
        }];
        assert_eq!(parse_assignment(&words), None);
    }

    #[test]
    fn underscore_and_digits_allowed_after_first_char() {
        let words = vec![expandable("_my_var2=value")];
        let (name, _) = parse_assignment(&words).unwrap();
        assert_eq!(name, "_my_var2");
    }
}

#[cfg(test)]
mod glob_tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Fresh on-disk fixture per call: a unique temp dir with a fixed set
    /// of entries (a.txt b.txt c.log, subdir `sub` holding 1.md/2.md, and
    /// a dotfile). Absolute patterns everywhere, so tests never chdir
    /// (which would race across parallel tests); a unique dir per call
    /// means tests can't observe each other's fixtures either.
    fn fixture() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("lush-glob-{}-{}", std::process::id(), id));
        fs::create_dir_all(&dir).unwrap();
        for name in ["a.txt", "b.txt", "c.log"] {
            fs::write(dir.join(name), "").unwrap();
        }
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/1.md"), "").unwrap();
        fs::write(dir.join("sub/2.md"), "").unwrap();
        fs::write(dir.join(".hidden"), "").unwrap();
        dir
    }

    fn no_lookup(_name: &str) -> Option<String> {
        None
    }

    fn var_lookup(name: &str) -> Option<String> {
        (name == "PAT").then(|| format!("{}/*.txt", fixture().display()))
    }

    fn expandable(s: &str) -> Word {
        Word {
            parts: vec![WordPart::Expandable(s.to_string())],
        }
    }

    fn names_of(matches: Vec<String>) -> Vec<String> {
        matches.iter().map(|m| {
            m.rsplit('/').next().unwrap().to_string()
        }).collect()
    }

    #[test]
    fn star_expands_and_sorts() {
        let dir = fixture();
        let word = expandable(&format!("{}/[ab].txt", dir.display()));
        assert_eq!(
            glob_expand_word(&word, &no_lookup),
            vec![dir.join("a.txt").to_string_lossy().to_string(),
                 dir.join("b.txt").to_string_lossy().to_string()]
        );
    }

    #[test]
    fn question_mark_matches_exactly_one_char() {
        let dir = fixture();
        // ? consumes exactly one character, so ?.log hits the 5-char c.log.
        let word = expandable(&format!("{}/?.log", dir.display()));
        assert_eq!(
            names_of(glob_expand_word(&word, &no_lookup)),
            vec!["c.log"]
        );
        // Requiring two chars where only one exists finds nothing.
        let word = expandable(&format!("{}/??.log", dir.display()));
        assert_eq!(glob_expand_word(&word, &no_lookup), Vec::<String>::new());
        // a?txt matches the 5-char a.txt.
        let word = expandable(&format!("{}/a?txt", dir.display()));
        assert_eq!(
            names_of(glob_expand_word(&word, &no_lookup)),
            vec!["a.txt"]
        );
    }

    #[test]
    fn ranges_and_negation_work() {
        let dir = fixture();
        let word = expandable(&format!("{}/[!ab]*", dir.display()));
        let matched = names_of(glob_expand_word(&word, &no_lookup));
        // c.log and sub (dotfile excluded by *), NOT a.txt/b.txt.
        assert_eq!(matched, vec!["c.log", "sub"]);
    }

    #[test]
    fn quoted_star_never_expands() {
        let dir = fixture();
        // Single-quoted '*' is Literal, so there's no unquoted wildcard
        // anywhere in the word and no expansion happens.
        let mut parts = vec![WordPart::Literal(format!("{}/", dir.display()))];
        parts.push(WordPart::Literal("*".to_string()));
        parts.push(WordPart::Expandable(".txt".to_string()));
        let word = Word { parts };
        assert_eq!(glob_expand_word(&word, &no_lookup), Vec::<String>::new());
    }

    #[test]
    fn double_quoted_star_never_expands() {
        // Regression for the bug the integration suite caught first:
        // "echo \"*.txt\"" used to expand because double-quoted runs were
        // stored indistinguishably from unquoted ones. DoubleQuoted text
        // is variable-expandable but glob-inert.
        let dir = fixture();
        let mut parts = vec![WordPart::Expandable(format!("{}/", dir.display()))];
        parts.push(WordPart::DoubleQuoted("*".to_string()));
        parts.push(WordPart::Expandable(".txt".to_string()));
        let word = Word { parts };
        assert_eq!(glob_expand_word(&word, &no_lookup), Vec::<String>::new());

        // Mixed quoting: only the UNQUOTED star is a wildcard here, and
        // it matches any name ending in a literal '*'.
        fs::write(dir.join("end*"), "").unwrap();
        let mut parts = vec![WordPart::Expandable(format!("{}/", dir.display()))];
        parts.push(WordPart::Expandable("*".to_string()));
        parts.push(WordPart::DoubleQuoted("*".to_string()));
        let word = Word { parts };
        assert_eq!(
            names_of(glob_expand_word(&word, &no_lookup)),
            vec!["end*"]
        );
    }

    #[test]
    fn variable_produced_pattern_globs_like_bash() {
        let dir = fixture();
        let word = expandable("$PAT");
        let matches = glob_expand_word(&word, &var_lookup);
        assert_eq!(names_of(matches), vec!["a.txt", "b.txt"]);

        // Same via ${VAR}: expanding to a path alone carries no wildcard,
        // so the pattern needs its own `*` to list a directory.
        let word = expandable("${HOME_OVERRIDE}/*");
        let matches = glob_expand_word(
            &word,
            &|n| (n == "HOME_OVERRIDE").then(|| format!("{}", dir.display())),
        );
        assert_eq!(names_of(matches), vec!["a.txt", "b.txt", "c.log", "sub"]);
    }

    #[test]
    fn subdirectory_patterns_walk_components() {
        let dir = fixture();
        let word = expandable(&format!("{}/sub/*.md", dir.display()));
        assert_eq!(
            names_of(glob_expand_word(&word, &no_lookup)),
            vec!["1.md", "2.md"]
        );

        // Intermediate component must be a directory: matching a file
        // mid-pattern yields nothing.
        let word = expandable(&format!("{}/a.txt/*.md", dir.display()));
        assert_eq!(glob_expand_word(&word, &no_lookup), Vec::<String>::new());
    }

    #[test]
    fn trailing_slash_means_dirs_only() {
        let dir = fixture();
        // */ lists directories only, keeping the trailing slash on results.
        let word = expandable(&format!("{}/*/", dir.display()));
        assert_eq!(
            glob_expand_word(&word, &no_lookup),
            vec![format!("{}/sub/", dir.display())]
        );
        // An active pattern that matches a FILE is filtered out by the slash.
        let word = expandable(&format!("{}/a*txt/", dir.display()));
        assert_eq!(glob_expand_word(&word, &no_lookup), Vec::<String>::new());
        // A fully literal path (no wildcard anywhere) gets no filesystem
        // consultation at all — bash parity: expansion needs a pattern.
        let word = expandable(&format!("{}/sub/", dir.display()));
        assert_eq!(glob_expand_word(&word, &no_lookup), Vec::<String>::new());
    }

    #[test]
    fn dotfiles_need_explicit_leading_dot() {
        let dir = fixture();
        let word = expandable(&format!("{}/*", dir.display()));
        let matched = names_of(glob_expand_word(&word, &no_lookup));
        assert!(!matched.contains(&".hidden".to_string()));

        let word = expandable(&format!("{}/.*", dir.display()));
        assert_eq!(
            names_of(glob_expand_word(&word, &no_lookup)),
            vec![".hidden"]
        );
    }

    #[test]
    fn unmatched_pattern_returns_empty_for_literal_fallback() {
        let dir = fixture();
        let word = expandable(&format!("{}/zzz*.txt", dir.display()));
        assert_eq!(glob_expand_word(&word, &no_lookup), Vec::<String>::new());
    }

    #[test]
    fn unterminated_bracket_is_a_literal_bracket() {
        let dir = fixture();
        // No closing ']': bash treats '[' as an ordinary character, so
        // this looks for files literally named "[ab" — none exist here.
        let word = expandable(&format!("{}/[ab", dir.display()));
        assert_eq!(glob_expand_word(&word, &no_lookup), Vec::<String>::new());
    }
}
