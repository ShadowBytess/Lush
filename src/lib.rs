#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Word(String),
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
    Eof,
}

pub fn lex(input: &str) -> Vec<Token> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote_char: Option<char> = None;
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        if let Some(qc) = quote_char {
            if c == qc {
                quote_char = None;
            } else {
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
            flush_word(&mut current, &mut tokens);
            i += 1;
        }
        '&' if chars.get(i + 1) == Some(&'&') => {
            flush_word(&mut current, &mut tokens);
            tokens.push(Token::And);
            i += 2;
        }
        '|' if chars.get(i + 1) == Some(&'|') => {
            flush_word(&mut current, &mut tokens);
            tokens.push(Token::Or);
            i += 2;
        }
        '|' => {
            flush_word(&mut current, &mut tokens);
            tokens.push(Token::Pipe);
            i += 1;
        }
        ';' | '\n' => {

            flush_word(&mut current, &mut tokens);
            tokens.push(Token::Semicolon);
            i += 1;
        }
        '>' if chars.get(i + 1) == Some(&'>') => {
            flush_word(&mut current, &mut tokens);
            tokens.push(Token::RedirectAppend);
            i += 2;
        }
        '>' => {
            flush_word(&mut current, &mut tokens);
            tokens.push(Token::RedirectOut);
            i += 1;
        }
        '<' => {
            flush_word(&mut current, &mut tokens);
            tokens.push(Token::RedirectIn);
            i += 1;
        }

        _ => {
            current.push(c);
            i += 1;
        }
    }
}

flush_word(&mut current, &mut tokens);
tokens.push(Token::Eof);
tokens
}

fn flush_word(current: &mut String, tokens: &mut Vec<Token>) {
    if !current.is_empty() {
        tokens.push(Token::Word(std::mem::take(current)));
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Command(SimpleCommand),
    Pipeline(Vec<SimpleCommand>),
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Sequence(Box<Node>, Box<Node>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SimpleCommand {
    pub words: Vec<String>,
    pub redirects: Vec<Redirect>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Redirect {
    In(String),
    Out(String),
    Append(String),
}

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

    fn expect_word(&mut self, op: &str) -> Result<String, String> {
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

    fn cmd(words: &[&str]) -> SimpleCommand {
        SimpleCommand {
            words: words.iter().map(|s| s.to_string()).collect(),
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
                       words: vec!["echo".into(), "hi".into()],
                                      redirects: vec![Redirect::Out("out.txt".into())],
                   }))
        );
    }

    #[test]
    fn redirect_glued_no_whitespace() {

        assert_eq!(parse("echo hi>out.txt").unwrap(), parse("echo hi > out.txt").unwrap());
    }

    #[test]
    fn and_or_left_associative() {
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

    #[test]
    fn simple_command() {
        let tokens = lex("echo hello world");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into()),
                   Token::Word("hello".into()),
                   Token::Word("world".into()),
                   Token::Eof,
            ]
        );
    }

    #[test]
    fn pipe_inside_quotes_is_not_a_pipe() {
        let tokens = lex(r#"echo "hello | world""#);
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into()),
                   Token::Word("hello | world".into()),
                   Token::Eof,
            ]
        );
    }

    #[test]
    fn redirect_without_whitespace() {
        let tokens = lex("echo hello>out.txt");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into()),
                   Token::Word("hello".into()),
                   Token::RedirectOut,
                   Token::Word("out.txt".into()),
                   Token::Eof,
            ]
        );
    }

    #[test]
    fn redirect_with_whitespace_still_works() {
        let tokens = lex("echo hello > out.txt");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into()),
                   Token::Word("hello".into()),
                   Token::RedirectOut,
                   Token::Word("out.txt".into()),
                   Token::Eof,
            ]
        );
    }

    #[test]
    fn append_redirect_glued() {
        let tokens = lex("echo hi>>out.txt");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into()),
                   Token::Word("hi".into()),
                   Token::RedirectAppend,
                   Token::Word("out.txt".into()),
                   Token::Eof,
            ]
        );
    }

    #[test]
    fn input_redirect_glued() {
        let tokens = lex("sort <in.txt");
        assert_eq!(
            tokens,
            vec![
                Token::Word("sort".into()),
                   Token::RedirectIn,
                   Token::Word("in.txt".into()),
                   Token::Eof,
            ]
        );
    }

    #[test]
    fn pipeline() {
        let tokens = lex("cat file.txt | grep foo | wc -l");
        assert_eq!(
            tokens,
            vec![
                Token::Word("cat".into()),
                   Token::Word("file.txt".into()),
                   Token::Pipe,
                   Token::Word("grep".into()),
                   Token::Word("foo".into()),
                   Token::Pipe,
                   Token::Word("wc".into()),
                   Token::Word("-l".into()),
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
                Token::Word("true".into()),
                   Token::And,
                   Token::Word("echo".into()),
                   Token::Word("a".into()),
                   Token::Or,
                   Token::Word("echo".into()),
                   Token::Word("b".into()),
                   Token::Semicolon,
                   Token::Word("echo".into()),
                   Token::Word("c".into()),
                   Token::Eof,
            ]
        );
    }

    #[test]
    fn single_and_double_quotes() {
        let tokens = lex(r#"echo 'single' "double""#);
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into()),
                   Token::Word("single".into()),
                   Token::Word("double".into()),
                   Token::Eof,
            ]
        );
    }

    #[test]
    fn adjacent_quoted_and_unquoted_merge_into_one_word() {
        let tokens = lex(r#"ab"c d"ef"#);
        assert_eq!(tokens, vec![Token::Word("abc def".into()), Token::Eof]);
    }

    #[test]
    fn empty_input_is_just_eof() {
        assert_eq!(lex(""), vec![Token::Eof]);
    }

    #[test]
    fn lone_ampersand_is_literal_for_now() {
        let tokens = lex("echo a & b");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into()),
                   Token::Word("a".into()),
                   Token::Word("&".into()),
                   Token::Word("b".into()),
                   Token::Eof,
            ]
        );
    }

    #[test]
    fn embedded_newline_terminates_a_statement() {
        let tokens = lex("echo work\ntrue");
        assert_eq!(
            tokens,
            vec![
                Token::Word("echo".into()),
                   Token::Word("work".into()),
                   Token::Semicolon,
                   Token::Word("true".into()),
                   Token::Eof,
            ]
        );
    }
}
