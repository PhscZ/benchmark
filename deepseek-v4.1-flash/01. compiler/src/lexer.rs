//! Byte-oriented lexer.  Operates on raw bytes (never `char`) so that non-ASCII
//! bytes inside string literals, character literals and comments pass through
//! unchanged.

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Num(i64),
    Str(Vec<u8>),
    Punct(String),
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub line: usize,
}

/// Longest match wins, so multi-character punctuators must precede the single
/// characters they start with.
const PUNCTS: &[&str] = &[
    ">>=", "<<=", "...", "==", "!=", "<=", ">=", "&&", "||", "++", "--", "+=", "-=", "*=", "/=",
    "%=", "&=", "|=", "^=", "<<", ">>", "->", "+", "-", "*", "/", "%", "&", "|", "^", "~", "!",
    "<", ">", "=", "(", ")", "[", "]", "{", "}", ",", ";", ":", "?", ".",
];

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}

fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

pub fn tokenize(src: &[u8]) -> Result<Vec<Token>, String> {
    let mut lx = Lexer {
        src,
        pos: 0,
        line: 1,
    };
    // Skip a UTF-8 byte order mark if present.
    if src.len() >= 3 && src[0] == 0xEF && src[1] == 0xBB && src[2] == 0xBF {
        lx.pos = 3;
    }
    let mut out = Vec::new();
    loop {
        let t = lx.next_token()?;
        let eof = matches!(t.tok, Tok::Eof);
        out.push(t);
        if eof {
            break;
        }
    }
    Ok(out)
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: usize,
}

impl<'a> Lexer<'a> {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, n: usize) -> Option<u8> {
        self.src.get(self.pos + n).copied()
    }

    fn skip_trivia(&mut self) -> Result<(), String> {
        loop {
            match self.peek() {
                Some(b'\n') => {
                    self.line += 1;
                    self.pos += 1;
                }
                // Any other control byte / space is whitespace.  This also
                // tolerates stray NUL bytes in otherwise-valid input.
                Some(c) if c <= b' ' || c == 0x7f => self.pos += 1,
                Some(b'/') if self.peek_at(1) == Some(b'/') => {
                    self.pos += 2;
                    while let Some(c) = self.peek() {
                        if c == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                Some(b'/') if self.peek_at(1) == Some(b'*') => {
                    self.pos += 2;
                    loop {
                        match self.peek() {
                            None => return Err("unterminated /* comment".to_string()),
                            Some(b'\n') => {
                                self.line += 1;
                                self.pos += 1;
                            }
                            Some(b'*') if self.peek_at(1) == Some(b'/') => {
                                self.pos += 2;
                                break;
                            }
                            Some(_) => self.pos += 1,
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn next_token(&mut self) -> Result<Token, String> {
        self.skip_trivia()?;
        let line = self.line;
        let c = match self.peek() {
            None => return Ok(Token { tok: Tok::Eof, line }),
            Some(c) => c,
        };
        if c.is_ascii_digit() {
            return self.read_number(line);
        }
        if c == b'"' {
            return self.read_string(line);
        }
        if c == b'\'' {
            return self.read_char(line);
        }
        if is_ident_start(c) {
            let start = self.pos;
            while let Some(c) = self.peek() {
                if is_ident_char(c) {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            let s = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
            return Ok(Token {
                tok: Tok::Ident(s),
                line,
            });
        }
        for p in PUNCTS {
            let pb = p.as_bytes();
            if self.src.len() >= self.pos + pb.len()
                && &self.src[self.pos..self.pos + pb.len()] == pb
            {
                self.pos += pb.len();
                return Ok(Token {
                    tok: Tok::Punct((*p).to_string()),
                    line,
                });
            }
        }
        Err(format!(
            "line {}: unexpected character (0x{:02x})",
            line, c
        ))
    }

    fn read_number(&mut self, line: usize) -> Result<Token, String> {
        let mut radix = 10u32;
        if self.peek() == Some(b'0') {
            match self.peek_at(1) {
                Some(b'x') | Some(b'X') => {
                    radix = 16;
                    self.pos += 2;
                }
                Some(c) if c.is_ascii_digit() => radix = 8,
                _ => {}
            }
        }
        let ds = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.src[ds..self.pos])
            .map_err(|_| format!("line {}: malformed integer literal", line))?;
        let digits: String = text.chars().take_while(|c| c.is_digit(radix)).collect();
        if digits.is_empty() {
            return Err(format!("line {}: malformed integer literal", line));
        }
        let v = u64::from_str_radix(&digits, radix)
            .map_err(|_| format!("line {}: integer literal out of range", line))?;
        Ok(Token {
            tok: Tok::Num(v as i64),
            line,
        })
    }

    fn read_string(&mut self, line: usize) -> Result<Token, String> {
        self.pos += 1; // opening quote
        let mut out = Vec::new();
        loop {
            let c = match self.peek() {
                None => return Err(format!("line {}: unterminated string literal", line)),
                Some(c) => c,
            };
            match c {
                b'"' => {
                    self.pos += 1;
                    break;
                }
                b'\n' => return Err(format!("line {}: newline in string literal", line)),
                b'\\' => {
                    self.pos += 1;
                    let b = self.read_escape(line)?;
                    out.push(b);
                }
                _ => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
        Ok(Token {
            tok: Tok::Str(out),
            line,
        })
    }

    fn read_char(&mut self, line: usize) -> Result<Token, String> {
        self.pos += 1; // opening quote
        let c = match self.peek() {
            None => return Err(format!("line {}: unterminated character literal", line)),
            Some(c) => c,
        };
        let v: i64 = if c == b'\\' {
            self.pos += 1;
            self.read_escape(line)? as i64
        } else if c == b'\'' {
            return Err(format!("line {}: empty character literal", line));
        } else {
            self.pos += 1;
            c as i64
        };
        if self.peek() != Some(b'\'') {
            return Err(format!("line {}: unterminated character literal", line));
        }
        self.pos += 1;
        Ok(Token {
            tok: Tok::Num(v),
            line,
        })
    }

    fn read_escape(&mut self, line: usize) -> Result<u8, String> {
        let c = match self.peek() {
            None => return Err(format!("line {}: truncated escape sequence", line)),
            Some(c) => c,
        };
        self.pos += 1;
        let v = match c {
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'a' => 7,
            b'b' => 8,
            b'f' => 12,
            b'v' => 11,
            b'\\' => b'\\',
            b'\'' => b'\'',
            b'"' => b'"',
            b'?' => b'?',
            b'0'..=b'7' => {
                let mut v = (c - b'0') as u32;
                for _ in 0..2 {
                    match self.peek() {
                        Some(d) if (b'0'..=b'7').contains(&d) => {
                            v = v * 8 + (d - b'0') as u32;
                            self.pos += 1;
                        }
                        _ => break,
                    }
                }
                v as u8
            }
            b'x' => {
                let mut v = 0u32;
                let mut n = 0;
                while let Some(d) = self.peek() {
                    let dv = match d {
                        b'0'..=b'9' => (d - b'0') as u32,
                        b'a'..=b'f' => (d - b'a' + 10) as u32,
                        b'A'..=b'F' => (d - b'A' + 10) as u32,
                        _ => break,
                    };
                    v = v.wrapping_mul(16).wrapping_add(dv);
                    self.pos += 1;
                    n += 1;
                }
                if n == 0 {
                    return Err(format!("line {}: \\x escape needs at least one digit", line));
                }
                v as u8
            }
            // Unknown escapes keep the escaped byte verbatim, like most C
            // compilers do as an extension.
            other => other,
        };
        Ok(v)
    }
}
