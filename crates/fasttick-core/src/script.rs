//! A syntax check for the script language of BT.CPP 4.9.0.
//!
//! BT.CPP parses each pre-condition and post-condition script when it
//! creates the tree (`BehaviorTreeFactory::instantiateTreeNode`), and so do
//! the constructors of the `Script`, `ScriptCondition` and `Precondition`
//! nodes. A script that does not parse stops the creation of the tree.
//!
//! This module is a copy of the tokenizer (`src/script_tokenizer.cpp`) and
//! of the Pratt parser (`ScriptParser` in `src/script_parser.cpp`) of BT.CPP
//! 4.9.0, without the evaluation. The error messages are the messages of
//! BT.CPP. Positions are byte offsets in the script.
//!
//! ```
//! use fasttick_core::script::check_script;
//!
//! assert!(check_script("flag == true && count < 3").is_ok());
//! assert!(check_script("a := 1; b := a + 2").is_ok());
//! let err = check_script("x == (1 + 2").unwrap_err();
//! assert_eq!(err.message, "Parse error at position 11: expected ')' (got '')");
//! ```

use std::fmt;
use std::ops::Range;

/// The kind of a token (`BT::Scripting::TokenType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// A decimal or hexadecimal integer.
    Integer,
    /// A real number.
    Real,
    /// A string in single quotes (the token text has no quotes).
    String,
    /// `true` or `false`.
    Boolean,
    /// A name (it can start with `@`).
    Identifier,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `..` (string concatenation)
    DotDot,
    /// `&`
    Ampersand,
    /// `|`
    Pipe,
    /// `^`
    Caret,
    /// `~`
    Tilde,
    /// `&&`
    AmpAmp,
    /// `||`
    PipePipe,
    /// `!`
    Bang,
    /// `==`
    EqualEqual,
    /// `!=`
    BangEqual,
    /// `<`
    Less,
    /// `>`
    Greater,
    /// `<=`
    LessEqual,
    /// `>=`
    GreaterEqual,
    /// `:=`
    ColonEqual,
    /// `=`
    Equal,
    /// `+=`
    PlusEqual,
    /// `-=`
    MinusEqual,
    /// `*=`
    StarEqual,
    /// `/=`
    SlashEqual,
    /// `?`
    Question,
    /// `:`
    Colon,
    /// `(`
    LeftParen,
    /// `)`
    RightParen,
    /// `;`
    Semicolon,
    /// The end of the script.
    EndOfInput,
    /// A character or a number that is not correct.
    Error,
}

/// One token: its kind and its text (a byte range of the script).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The kind.
    pub kind: TokenKind,
    /// The text, as a byte range. For a string, the range has no quotes.
    pub text: Range<usize>,
    /// The byte offset where the token starts.
    pub pos: usize,
}

/// A script that BT.CPP does not accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError {
    /// The message of BT.CPP, for example `Expected operand at position 4
    /// (got ')')`.
    pub message: String,
    /// The byte offset of the problem.
    pub position: usize,
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ScriptError {}

/// The message of BT.CPP for a script without statements.
pub const EMPTY_SCRIPT: &str = "Empty Script";

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'@'
}

fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn consume_trailing_garbage(source: &[u8], i: &mut usize) {
    while *i < source.len() && (is_ident_char(source[*i]) || source[*i] == b'.') {
        *i += 1;
    }
}

/// Scans a hexadecimal number after `0x`. Returns true on an error.
fn scan_hex(source: &[u8], i: &mut usize) -> bool {
    let mut error = false;
    *i += 2;
    if *i >= source.len() || !source[*i].is_ascii_hexdigit() {
        error = true;
    } else {
        while *i < source.len() && source[*i].is_ascii_hexdigit() {
            *i += 1;
        }
    }
    if *i < source.len() && (source[*i] == b'.' || is_ident_char(source[*i])) {
        error = true;
        consume_trailing_garbage(source, i);
    }
    error
}

/// Scans a decimal number. Returns (is_real, has_error).
fn scan_decimal(source: &[u8], i: &mut usize) -> (bool, bool) {
    let len = source.len();
    let (mut real, mut error) = (false, false);
    while *i < len && source[*i].is_ascii_digit() {
        *i += 1;
    }
    if *i < len && source[*i] == b'.' {
        if *i + 1 < len && source[*i + 1] == b'.' {
            // "65.." is Integer("65") and DotDot.
        } else if *i + 1 < len && source[*i + 1].is_ascii_digit() {
            real = true;
            *i += 1;
            while *i < len && source[*i].is_ascii_digit() {
                *i += 1;
            }
        } else {
            error = true;
            *i += 1;
            consume_trailing_garbage(source, i);
        }
    }
    if !error && *i < len && (source[*i] == b'e' || source[*i] == b'E') {
        real = true;
        *i += 1;
        if *i < len && (source[*i] == b'+' || source[*i] == b'-') {
            *i += 1;
        }
        if *i >= len || !source[*i].is_ascii_digit() {
            error = true;
        } else {
            while *i < len && source[*i].is_ascii_digit() {
                *i += 1;
            }
        }
    }
    if !error && *i < len && is_ident_start(source[*i]) {
        error = true;
        while *i < len && is_ident_char(source[*i]) {
            *i += 1;
        }
    }
    (real, error)
}

fn two_char_op(c: u8, next: u8) -> Option<TokenKind> {
    Some(match (c, next) {
        (b'.', b'.') => TokenKind::DotDot,
        (b'&', b'&') => TokenKind::AmpAmp,
        (b'|', b'|') => TokenKind::PipePipe,
        (b'=', b'=') => TokenKind::EqualEqual,
        (b'!', b'=') => TokenKind::BangEqual,
        (b'<', b'=') => TokenKind::LessEqual,
        (b'>', b'=') => TokenKind::GreaterEqual,
        (b':', b'=') => TokenKind::ColonEqual,
        (b'+', b'=') => TokenKind::PlusEqual,
        (b'-', b'=') => TokenKind::MinusEqual,
        (b'*', b'=') => TokenKind::StarEqual,
        (b'/', b'=') => TokenKind::SlashEqual,
        _ => return None,
    })
}

fn one_char_op(c: u8) -> TokenKind {
    match c {
        b'+' => TokenKind::Plus,
        b'-' => TokenKind::Minus,
        b'*' => TokenKind::Star,
        b'/' => TokenKind::Slash,
        b'&' => TokenKind::Ampersand,
        b'|' => TokenKind::Pipe,
        b'^' => TokenKind::Caret,
        b'~' => TokenKind::Tilde,
        b'!' => TokenKind::Bang,
        b'<' => TokenKind::Less,
        b'>' => TokenKind::Greater,
        b'=' => TokenKind::Equal,
        b'?' => TokenKind::Question,
        b':' => TokenKind::Colon,
        b'(' => TokenKind::LeftParen,
        b')' => TokenKind::RightParen,
        b';' => TokenKind::Semicolon,
        _ => TokenKind::Error,
    }
}

/// Splits a script into tokens (`BT::Scripting::tokenize`). The last token
/// is [`TokenKind::EndOfInput`].
pub fn tokenize(script: &str) -> Vec<Token> {
    let source = script.as_bytes();
    let len = source.len();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < len {
        let c = source[i];
        if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
            continue;
        }
        let start = i;
        if c == b'\'' {
            i += 1;
            while i < len && source[i] != b'\'' {
                i += 1;
            }
            if i < len {
                tokens.push(Token {
                    kind: TokenKind::String,
                    text: start + 1..i,
                    pos: start,
                });
                i += 1;
            } else {
                tokens.push(Token {
                    kind: TokenKind::Error,
                    text: start..i,
                    pos: start,
                });
            }
            continue;
        }
        if c.is_ascii_digit() {
            let hex = c == b'0' && i + 1 < len && matches!(source[i + 1], b'x' | b'X');
            let (real, error) = if hex {
                (false, scan_hex(source, &mut i))
            } else {
                scan_decimal(source, &mut i)
            };
            let kind = if error {
                TokenKind::Error
            } else if real {
                TokenKind::Real
            } else {
                TokenKind::Integer
            };
            tokens.push(Token {
                kind,
                text: start..i,
                pos: start,
            });
            continue;
        }
        if is_ident_start(c) {
            i += 1;
            while i < len && is_ident_char(source[i]) {
                i += 1;
            }
            let text = &source[start..i];
            let kind = if text == b"true" || text == b"false" {
                TokenKind::Boolean
            } else {
                TokenKind::Identifier
            };
            tokens.push(Token {
                kind,
                text: start..i,
                pos: start,
            });
            continue;
        }
        if i + 1 < len
            && let Some(kind) = two_char_op(c, source[i + 1])
        {
            tokens.push(Token {
                kind,
                text: start..start + 2,
                pos: start,
            });
            i += 2;
            continue;
        }
        tokens.push(Token {
            kind: one_char_op(c),
            text: start..start + 1,
            pos: start,
        });
        i += 1;
    }
    tokens.push(Token {
        kind: TokenKind::EndOfInput,
        text: i..i,
        pos: i,
    });
    tokens
}

const ASSIGNMENT_BP: i32 = 2;
const TERNARY_BP: i32 = 4;
const COMPARISON_BP: i32 = 10;
const MUL_DIV_BP: i32 = 18;
const PREFIX_BP: i32 = 20;

/// The deepest nesting that the check accepts. BT.CPP has no limit (deeper
/// scripts overflow its stack); FastTick stops here to protect its own
/// stack.
pub const MAX_DEPTH: usize = 500;

/// The left binding power of an infix operator (`ScriptParser::leftBP`).
fn left_bp(kind: TokenKind) -> Option<i32> {
    use TokenKind::*;
    Some(match kind {
        ColonEqual | Equal | PlusEqual | MinusEqual | StarEqual | SlashEqual => ASSIGNMENT_BP,
        Question => TERNARY_BP,
        PipePipe => 6,
        AmpAmp => 8,
        EqualEqual | BangEqual | Less | Greater | LessEqual | GreaterEqual => COMPARISON_BP,
        Pipe | Caret => 12,
        Ampersand => 14,
        Plus | Minus | DotDot => 16,
        Star | Slash => MUL_DIV_BP,
        _ => return None,
    })
}

fn is_comparison(kind: TokenKind) -> bool {
    use TokenKind::*;
    matches!(
        kind,
        EqualEqual | BangEqual | Less | Greater | LessEqual | GreaterEqual
    )
}

fn is_assignment(kind: TokenKind) -> bool {
    use TokenKind::*;
    matches!(
        kind,
        ColonEqual | Equal | PlusEqual | MinusEqual | StarEqual | SlashEqual
    )
}

/// True if `text` converts to a `double` with `std::from_chars`, as
/// `convertFromString<double>` needs. A value that is too large is an
/// error (`ERANGE`), and so is a value that is so small that it becomes
/// zero. Subnormal values are correct (the BT.CPP oracle accepts `1e-310`).
fn real_converts(text: &str) -> bool {
    let Ok(value) = text.parse::<f64>() else {
        return false;
    };
    if !value.is_finite() {
        return false;
    }
    let mantissa_is_zero = text
        .split(['e', 'E'])
        .next()
        .is_some_and(|m| m.bytes().all(|b| b == b'0' || b == b'.'));
    !(value == 0.0 && !mantissa_is_zero)
}

struct Parser<'a> {
    script: &'a str,
    tokens: Vec<Token>,
    current: usize,
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> &Token {
        &self.tokens[self.current]
    }

    fn advance(&mut self) {
        if self.peek().kind != TokenKind::EndOfInput {
            self.current += 1;
        }
    }

    fn text(&self, token: &Token) -> String {
        String::from_utf8_lossy(&self.script.as_bytes()[token.text.clone()]).into_owned()
    }

    fn error(&self, message: String, position: usize) -> ScriptError {
        ScriptError { message, position }
    }

    fn expect(&mut self, kind: TokenKind, what: &str) -> Result<(), ScriptError> {
        let token = self.peek().clone();
        if token.kind != kind {
            return Err(self.error(
                format!(
                    "Parse error at position {}: {what} (got '{}')",
                    token.pos,
                    self.text(&token)
                ),
                token.pos,
            ));
        }
        self.advance();
        Ok(())
    }

    fn parse_all(&mut self) -> Result<usize, ScriptError> {
        let mut statements = 0;
        while self.peek().kind != TokenKind::EndOfInput {
            self.parse_expr(0)?;
            statements += 1;
            while self.peek().kind == TokenKind::Semicolon {
                self.advance();
            }
        }
        Ok(statements)
    }

    fn parse_prefix(&mut self) -> Result<(), ScriptError> {
        let token = self.peek().clone();
        match token.kind {
            TokenKind::Minus | TokenKind::Tilde | TokenKind::Bang => {
                self.advance();
                self.parse_expr(PREFIX_BP)
            }
            TokenKind::LeftParen => {
                self.advance();
                self.parse_expr(0)?;
                self.expect(TokenKind::RightParen, "expected ')'")
            }
            TokenKind::Real => {
                let text = self.text(&token);
                if !real_converts(&text) {
                    return Err(self.error(
                        format!("Can't convert string [{text}] to double"),
                        token.pos,
                    ));
                }
                self.advance();
                Ok(())
            }
            TokenKind::Boolean | TokenKind::Integer | TokenKind::String | TokenKind::Identifier => {
                self.advance();
                Ok(())
            }
            TokenKind::Error => Err(self.error(
                format!(
                    "Invalid token '{}' at position {}",
                    self.text(&token),
                    token.pos
                ),
                token.pos,
            )),
            _ => Err(self.error(
                format!(
                    "Expected operand at position {} (got '{}')",
                    token.pos,
                    self.text(&token)
                ),
                token.pos,
            )),
        }
    }

    fn parse_expr(&mut self, min_bp: i32) -> Result<(), ScriptError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            let position = self.peek().pos;
            return Err(self.error(
                format!("The script is nested too deeply (more than {MAX_DEPTH} levels)"),
                position,
            ));
        }
        let result = self.parse_expr_inner(min_bp);
        self.depth -= 1;
        result
    }

    fn parse_expr_inner(&mut self, min_bp: i32) -> Result<(), ScriptError> {
        self.parse_prefix()?;
        loop {
            let kind = self.peek().kind;
            let Some(lbp) = left_bp(kind) else {
                break;
            };
            if lbp < min_bp {
                break;
            }
            if is_assignment(kind) {
                self.advance();
                self.parse_expr(0)?;
                break;
            }
            if kind == TokenKind::Question {
                self.advance();
                self.parse_expr(0)?;
                self.expect(TokenKind::Colon, "expected ':' in ternary expression")?;
                self.parse_expr(TERNARY_BP)?;
                break;
            }
            if is_comparison(kind) {
                while is_comparison(self.peek().kind) {
                    self.advance();
                    self.parse_expr(COMPARISON_BP + 1)?;
                }
                continue;
            }
            self.advance();
            self.parse_expr(lbp + 1)?;
        }
        Ok(())
    }
}

/// Checks the syntax of a script like `BT::ValidateScript`. Returns the
/// number of statements, or the error of BT.CPP. A script without
/// statements (only white space) is the error [`EMPTY_SCRIPT`].
pub fn check_script(script: &str) -> Result<usize, ScriptError> {
    let mut parser = Parser {
        script,
        tokens: tokenize(script),
        current: 0,
        depth: 0,
    };
    let statements = parser.parse_all()?;
    if statements == 0 {
        return Err(ScriptError {
            message: EMPTY_SCRIPT.to_owned(),
            position: 0,
        });
    }
    Ok(statements)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(script: &str) -> Vec<TokenKind> {
        tokenize(script).into_iter().map(|t| t.kind).collect()
    }

    fn message(script: &str) -> String {
        check_script(script).unwrap_err().message
    }

    #[test]
    fn tokens() {
        use TokenKind::*;
        assert_eq!(
            kinds("a:=1; b += 0x1F .. 'x' @c"),
            [
                Identifier, ColonEqual, Integer, Semicolon, Identifier, PlusEqual, Integer, DotDot,
                String, Identifier, EndOfInput
            ]
        );
        assert_eq!(kinds("65..x"), [Integer, DotDot, Identifier, EndOfInput]);
        assert_eq!(kinds("1.5e-3"), [Real, EndOfInput]);
        assert_eq!(kinds("3foo"), [Error, EndOfInput]);
        assert_eq!(kinds("65."), [Error, EndOfInput]);
        assert_eq!(kinds("0x"), [Error, EndOfInput]);
        assert_eq!(kinds("'open"), [Error, EndOfInput]);
        assert_eq!(
            kinds("true false truex"),
            [Boolean, Boolean, Identifier, EndOfInput]
        );
        assert_eq!(kinds("#"), [Error, EndOfInput]);
    }

    #[test]
    fn accepted_scripts() {
        for script in [
            "flag == true",
            "a := 1; b := 2",
            "a := 1 b := 2",
            "x > 1 && y < 2 || !z",
            "1 < x < 3",
            "msg := 'a' .. 'b'",
            "c := flag ? 1 : 2",
            "@root_key += 1",
            "x := -(-1) * ~2 / 3",
            "a = b = c",
            "a;;",
            "e := 1.5e3",
        ] {
            assert!(check_script(script).is_ok(), "{script}");
        }
    }

    #[test]
    fn rejected_scripts() {
        assert_eq!(message(""), EMPTY_SCRIPT);
        assert_eq!(message("  \n\t"), EMPTY_SCRIPT);
        assert_eq!(message(";"), "Expected operand at position 0 (got ';')");
        assert_eq!(message("x == "), "Expected operand at position 5 (got '')");
        assert_eq!(
            message("(1"),
            "Parse error at position 2: expected ')' (got '')"
        );
        assert_eq!(
            message("a ? b"),
            "Parse error at position 5: expected ':' in ternary expression (got '')"
        );
        assert_eq!(message("x := 3foo"), "Invalid token '3foo' at position 5");
        assert_eq!(message("x == 'a"), "Invalid token ''a' at position 5");
        assert_eq!(message("x # 1"), "Invalid token '#' at position 2");
        assert_eq!(message("1e999"), "Can't convert string [1e999] to double");
        assert_eq!(message("1e-400"), "Can't convert string [1e-400] to double");
        assert!(
            check_script("1e-310").is_ok(),
            "subnormal values are correct"
        );
        let deep = "(".repeat(MAX_DEPTH + 10);
        assert!(message(&deep).contains("nested too deeply"));
    }
}
