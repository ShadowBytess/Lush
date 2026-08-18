//! Lush's core library. This is where the new lexer/parser/AST/executor
//! pipeline lives as it's built out, kept separate from `main.rs` so it's
//! directly unit-testable with `cargo test`.
//!
//! Status: Phase 2, Checkpoint 1. `Word` now tracks quote-kind per
//! segment (`WordPart::Literal` for single-quoted text, `WordPart::
//! Expandable` for everything else), which is what makes accurate `$VAR`
//! expansion possible in the next checkpoint: `echo '$HOME'` needs to
//! stay literal while `echo "$HOME"` and `echo $HOME` both expand, and
//! that distinction has to survive from lexing through to execution.
//! This checkpoint is a pure representation change: `main.rs` flattens
//! every `Word` straight back to a plain `String` via `Word::text()`, so
//! nothing about the shell's observable behavior changes yet.

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
    /// Marks the end of input. Every token stream ends with exactly one
    /// of these, so the parser never has to guess whether it's run off
    /// the end of the slice.
    Eof,
}

/// One piece of a word, tagged with whether it's subject to variable
/// expansion. A word only splits into multiple parts where its
/// quote-kind actually changes mid-word (`'abc'$HOME` splits into two;
/// `"abc"$HOME` doesn't, since double-quoted and unquoted text are both
/// expandable and can just merge).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WordPart {
    /// Came from inside single quotes. Never subject to `$VAR` expansion.
    Literal(String),
    /// Unquoted or double-quoted text. Subject to `$VAR`/`${VAR}`
    /// expansion once that pass exists (Phase 2, Checkpoint 2).
    Expandable(String),
}

impl WordPart {
    fn as_str(&self) -> &str {
        match self {
            WordPart::Literal(s) => s,
            WordPart::Expandable(s) => s,
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
    // Whether the run currently being built in `current` is literal
    // (came from single quotes) or expandable (everything else). Tracked
    // separately from `quote_char` because both "" and no-quote produce
    // the same (expandable) kind, only '' differs.
    let mut current_literal = false;
    let mut word_parts: Vec<WordPart> = Vec::new();
    let mut quote_char: Option<char> = None;
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        if let Some(qc) = quote_char {
            if c == qc {
                quote_char = None;
            } else {
                let literal = qc == '\'';
                if literal != current_literal {
                    flush_run(&mut current, current_literal, &mut word_parts);
                    current_literal = literal;
                }
                current.push(c);
            }
            i += 1;
            continue;
        }

        match c {
            '"' | '\'' => {
                quote_char = Some(c);
                i += 1;
            }
            ' ' | '\t' => {
                flush_word(&mut current, &mut current_literal, &mut word_parts, &mut tokens);
                i += 1;
            }
            ';' | '\n' => {
                flush_word(&mut current, &mut current_literal, &mut word_parts, &mut tokens);
                tokens.push(Token::Semicolon);
                i += 1;
            }
            '&' if chars.get(i + 1) == Some(&'&') => {
                flush_word(&mut current, &mut current_literal, &mut word_parts, &mut tokens);
                tokens.push(Token::And);
                i += 2;
            }
            '|' if chars.get(i + 1) == Some(&'|') => {
                flush_word(&mut current, &mut current_literal, &mut word_parts, &mut tokens);
                tokens.push(Token::Or);
                i += 2;
            }
            '|' => {
                flush_word(&mut current, &mut current_literal, &mut word_parts, &mut tokens);
                tokens.push(Token::Pipe);
                i += 1;
            }
            '>' if chars.get(i + 1) == Some(&'>') => {
                flush_word(&mut current, &mut current_literal, &mut word_parts, &mut tokens);
                tokens.push(Token::RedirectAppend);
                i += 2;
            }
            '>' => {
                flush_word(&mut current, &mut current_literal, &mut word_parts, &mut tokens);
                tokens.push(Token::RedirectOut);
                i += 1;
            }
            '<' => {
                flush_word(&mut current, &mut current_literal, &mut word_parts, &mut tokens);
                tokens.push(Token::RedirectIn);
                i += 1;
            }
            // A lone '&' (not doubled) falls through to here and becomes
            // literal word text. Background jobs (Priority 15) aren't
            // implemented yet, so this preserves today's behavior rather
            // than erroring on something the shell can't act on anyway.
            _ => {
                if current_literal {
                    flush_run(&mut current, current_literal, &mut word_parts);
                    current_literal = false;
                }
                current.push(c);
                i += 1;
            }
        }
    }

    flush_word(&mut current, &mut current_literal, &mut word_parts, &mut tokens);
    tokens.push(Token::Eof);
    tokens
}

/// Pushes the in-progress run onto `parts` (if non-empty) as a Literal or
/// Expandable part depending on `literal`, then clears `current`. Called
/// whenever the run's quote-kind is about to change, or the word itself
/// ends.
fn flush_run(current: &mut String, literal: bool, parts: &mut Vec<WordPart>) {
    if !current.is_empty() {
        let text = std::mem::take(current);
        parts.push(if literal {
            WordPart::Literal(text)
        } else {
            WordPart::Expandable(text)
        });
    }
}

/// Flushes the in-progress run, then pushes the accumulated parts as a
/// single Word token (if non-empty). Resets `current_literal` to false
/// afterward, every new word starts out unquoted until proven otherwise.
fn flush_word(
    current: &mut String,
    current_literal: &mut bool,
    parts: &mut Vec<WordPart>,
    tokens: &mut Vec<Token>,
) {
    flush_run(current, *current_literal, parts);
    if !parts.is_empty() {
        tokens.push(Token::Word(Word {
            parts: std::mem::take(parts),
        }));
    }
    *current_literal = false;
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
        assert_eq!(
            parse(r#"echo "hello | world""#).unwrap(),
            Some(Node::Command(cmd(&["echo", "hello | world"])))
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
        // become a Pipe token.
        let tokens = lex(r#"echo "hello | world""#);
        assert_eq!(tokens, vec![w("echo"), w("hello | world"), Token::Eof]);
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

    #[test]
    fn single_and_double_quotes() {
        // Single-quoted stays Literal, double-quoted is Expandable, this
        // is the whole point of the precise per-segment tracking chosen
        // for this checkpoint.
        let tokens = lex(r#"echo 'single' "double""#);
        assert_eq!(tokens, vec![w("echo"), lit("single"), w("double"), Token::Eof]);
    }

    #[test]
    fn adjacent_quoted_and_unquoted_merge_into_one_word() {
        // Matches real shell semantics: "ab" + quoted "c d" + "ef" glued
        // with no space between them is ONE word, "abc def". And since
        // double-quoted and unquoted are both Expandable, they merge into
        // a single WordPart rather than splitting, no quote-kind change
        // ever occurs across this word.
        let tokens = lex(r#"ab"c d"ef"#);
        assert_eq!(tokens, vec![w("abc def"), Token::Eof]);
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
}
