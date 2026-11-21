//! A small, strict shell tokenizer for labelling the agent's bash commands.
//!
//! It understands what agents actually write: words with quotes and escapes, the operators
//! `&&`, `||`, `;`, `|`, `&` and newlines, redirections (`2>&1`, `>/dev/null`, `> file`,
//! `<<'EOF'` heredocs) and comments. Anything it does not fully understand (command
//! substitution, subshells, process substitution, unbalanced quotes, an unfinished heredoc)
//! makes [`parse`] return `None`, so callers show the raw command instead of a guess. The
//! result is for display only; it is never a basis for permission decisions.

/// A command line: lists of pipelines, separated by `&&`, `||`, `;`, `&` or newlines.
pub type Script = Vec<Pipeline>;

/// Commands connected by `|`.
pub type Pipeline = Vec<Simple>;

/// One simple command: its words (quotes removed) and redirections.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Simple {
    pub words: Vec<String>,
    pub redirects: Vec<Redirect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Redirect {
    /// `>`, `>>`, `&>`, `>|` to a file (or `/dev/null`).
    Write { target: String },
    /// `< file`.
    Read { target: String },
    /// `2>&1`, `>&2`, `<&0`: between descriptors, no file.
    Duplicate,
    /// A heredoc (`<<EOF`) or here-string (`<<<`): text fed to the command.
    Input { body: String },
}

impl Redirect {
    /// Whether it writes to a real file (not `/dev/null` or a descriptor).
    pub fn writes_file(&self) -> Option<&str> {
        match self {
            Self::Write { target } if !is_null_device(target) => Some(target),
            _ => None,
        }
    }
}

fn is_null_device(path: &str) -> bool {
    matches!(path, "/dev/null" | "/dev/stdout" | "/dev/stderr")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    /// `&&`, `||`, `;`, `&` or a newline: ends a pipeline.
    Separator,
    Pipe,
    Redirect(Redirect),
}

/// Parses `source`; `None` when it uses syntax this tokenizer does not handle, or is
/// incomplete (e.g. still streaming).
pub fn parse(source: &str) -> Option<Script> {
    let tokens = Lexer::new(source).run()?;
    let mut script = Vec::new();
    let mut pipeline: Pipeline = Vec::new();
    let mut simple = Simple::default();
    for token in tokens {
        match token {
            Token::Word(word) => simple.words.push(word),
            Token::Redirect(redirect) => simple.redirects.push(redirect),
            Token::Pipe => {
                if simple.words.is_empty() {
                    return None;
                }
                pipeline.push(std::mem::take(&mut simple));
            }
            Token::Separator => {
                if simple.words.is_empty() {
                    // `a;` or blank lines are fine; `a | ;` or a lone redirect is not.
                    if !pipeline.is_empty() || !simple.redirects.is_empty() {
                        return None;
                    }
                    continue;
                }
                pipeline.push(std::mem::take(&mut simple));
                script.push(std::mem::take(&mut pipeline));
            }
        }
    }
    if simple.words.is_empty() {
        if !pipeline.is_empty() || !simple.redirects.is_empty() {
            return None;
        }
    } else {
        pipeline.push(simple);
        script.push(pipeline);
    }
    (!script.is_empty()).then_some(script)
}

struct Lexer<'a> {
    chars: Vec<char>,
    position: usize,
    tokens: Vec<Token>,
    /// The word being read, and whether any of it was quoted (`""` is a word).
    word: String,
    in_word: bool,
    quoted: bool,
    /// Heredocs whose bodies start after the next newline: (token index, delimiter, strip
    /// leading tabs).
    heredocs: Vec<(usize, String, bool)>,
    /// After `&&`, `||` or `|`: a command must follow.
    dangling: bool,
    _source: &'a str,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            chars: source.chars().collect(),
            position: 0,
            tokens: Vec::new(),
            word: String::new(),
            in_word: false,
            quoted: false,
            heredocs: Vec::new(),
            dangling: false,
            _source: source,
        }
    }

    fn peek(&self, offset: usize) -> Option<char> {
        self.chars.get(self.position + offset).copied()
    }

    fn finish_word(&mut self) {
        if self.in_word {
            self.dangling = false;
            self.tokens
                .push(Token::Word(std::mem::take(&mut self.word)));
            self.in_word = false;
            self.quoted = false;
        }
    }

    fn run(mut self) -> Option<Vec<Token>> {
        while let Some(c) = self.peek(0) {
            match c {
                ' ' | '\t' | '\r' => {
                    self.finish_word();
                    self.position += 1;
                }
                '\n' => {
                    self.finish_word();
                    self.position += 1;
                    self.tokens.push(Token::Separator);
                    self.read_heredoc_bodies()?;
                }
                '#' if !self.in_word => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.position += 1;
                    }
                }
                '\\' => {
                    self.position += 1;
                    match self.peek(0) {
                        // A line continuation.
                        Some('\n') => self.position += 1,
                        Some(c) => {
                            self.word.push(c);
                            self.in_word = true;
                            self.position += 1;
                        }
                        None => return None,
                    }
                }
                '\'' => {
                    self.position += 1;
                    self.in_word = true;
                    self.quoted = true;
                    loop {
                        match self.peek(0)? {
                            '\'' => break,
                            c => self.word.push(c),
                        }
                        self.position += 1;
                    }
                    self.position += 1;
                }
                '"' => {
                    self.position += 1;
                    self.in_word = true;
                    self.quoted = true;
                    loop {
                        match self.peek(0)? {
                            '"' => break,
                            '`' => return None,
                            '$' if self.peek(1) == Some('(') => return None,
                            '\\' => {
                                let next = self.peek(1)?;
                                if !matches!(next, '"' | '\\' | '$' | '`' | '\n') {
                                    self.word.push('\\');
                                }
                                if next != '\n' {
                                    self.word.push(next);
                                }
                                self.position += 1;
                            }
                            c => self.word.push(c),
                        }
                        self.position += 1;
                    }
                    self.position += 1;
                }
                // Command substitution, subshells, grouping: not handled.
                '`' | '(' | ')' => return None,
                '$' if matches!(self.peek(1), Some('(' | '\'')) => return None,
                '&' => {
                    self.finish_word();
                    match self.peek(1) {
                        Some('&') => {
                            self.position += 2;
                            self.tokens.push(Token::Separator);
                            self.dangling = true;
                        }
                        Some('>') => {
                            self.position += 2;
                            if self.peek(0) == Some('>') {
                                self.position += 1;
                            }
                            let target = self.redirect_target()?;
                            self.tokens
                                .push(Token::Redirect(Redirect::Write { target }));
                        }
                        _ => {
                            self.position += 1;
                            self.tokens.push(Token::Separator);
                        }
                    }
                }
                '|' => {
                    self.finish_word();
                    match self.peek(1) {
                        Some('|') => {
                            self.position += 2;
                            self.tokens.push(Token::Separator);
                        }
                        Some('&') => {
                            self.position += 2;
                            self.tokens.push(Token::Pipe);
                        }
                        _ => {
                            self.position += 1;
                            self.tokens.push(Token::Pipe);
                        }
                    }
                    self.dangling = true;
                }
                ';' => {
                    if self.peek(1) == Some(';') {
                        return None;
                    }
                    self.finish_word();
                    self.position += 1;
                    self.tokens.push(Token::Separator);
                }
                '>' | '<' => self.redirect()?,
                c => {
                    self.word.push(c);
                    self.in_word = true;
                    self.position += 1;
                }
            }
        }
        self.finish_word();
        // A heredoc whose body never came, or an operator still waiting for its command.
        if !self.heredocs.is_empty() || self.dangling {
            return None;
        }
        Some(self.tokens)
    }

    /// A redirection at `>` or `<`; a word of digits right before it is its descriptor.
    fn redirect(&mut self) -> Option<()> {
        if self.in_word {
            if self.quoted || !self.word.chars().all(|c| c.is_ascii_digit()) {
                self.finish_word();
            } else {
                // The descriptor number: not a word.
                self.word.clear();
                self.in_word = false;
            }
        }
        let c = self.peek(0)?;
        let next = self.peek(1);
        if matches!(next, Some('(')) {
            return None; // process substitution
        }
        if c == '>' {
            self.position += 1;
            match self.peek(0) {
                Some('>' | '|') => self.position += 1,
                Some('&') => {
                    self.position += 1;
                    self.descriptor_target()?;
                    self.tokens.push(Token::Redirect(Redirect::Duplicate));
                    return Some(());
                }
                _ => {}
            }
            let target = self.redirect_target()?;
            self.tokens
                .push(Token::Redirect(Redirect::Write { target }));
            return Some(());
        }
        // `<`
        self.position += 1;
        match self.peek(0) {
            Some('<') => {
                self.position += 1;
                if self.peek(0) == Some('<') {
                    self.position += 1;
                    let body = self.redirect_target()?;
                    self.tokens.push(Token::Redirect(Redirect::Input { body }));
                    return Some(());
                }
                let strip_tabs = self.peek(0) == Some('-');
                if strip_tabs {
                    self.position += 1;
                }
                let delimiter = self.redirect_target()?;
                let index = self.tokens.len();
                self.tokens.push(Token::Redirect(Redirect::Input {
                    body: String::new(),
                }));
                self.heredocs.push((index, delimiter, strip_tabs));
            }
            Some('&') => {
                self.position += 1;
                self.descriptor_target()?;
                self.tokens.push(Token::Redirect(Redirect::Duplicate));
            }
            Some('>') => return None,
            _ => {
                let target = self.redirect_target()?;
                self.tokens.push(Token::Redirect(Redirect::Read { target }));
            }
        }
        Some(())
    }

    /// The digits (or `-`) after `>&` / `<&`.
    fn descriptor_target(&mut self) -> Option<()> {
        let start = self.position;
        while self.peek(0).is_some_and(|c| c.is_ascii_digit() || c == '-') {
            self.position += 1;
        }
        (self.position > start).then_some(())
    }

    /// The word after a redirection operator (quotes removed).
    fn redirect_target(&mut self) -> Option<String> {
        while matches!(self.peek(0), Some(' ' | '\t')) {
            self.position += 1;
        }
        let mut target = String::new();
        while let Some(c) = self.peek(0) {
            match c {
                ' ' | '\t' | '\n' | ';' | '&' | '|' | '<' | '>' | '(' | ')' | '`' => break,
                '\'' => {
                    self.position += 1;
                    while let Some(c) = self.peek(0).filter(|c| *c != '\'') {
                        target.push(c);
                        self.position += 1;
                    }
                    self.peek(0)?;
                }
                '"' => {
                    self.position += 1;
                    while let Some(c) = self.peek(0).filter(|c| *c != '"') {
                        if c == '$' && self.peek(1) == Some('(') {
                            return None;
                        }
                        target.push(c);
                        self.position += 1;
                    }
                    self.peek(0)?;
                }
                '$' if self.peek(1) == Some('(') => return None,
                c => target.push(c),
            }
            self.position += 1;
        }
        (!target.is_empty()).then_some(target)
    }

    /// After a newline: the bodies of the heredocs started on the line before.
    fn read_heredoc_bodies(&mut self) -> Option<()> {
        for (index, delimiter, strip_tabs) in std::mem::take(&mut self.heredocs) {
            let mut body = String::new();
            loop {
                if self.position >= self.chars.len() {
                    return None;
                }
                let start = self.position;
                while self.peek(0).is_some_and(|c| c != '\n') {
                    self.position += 1;
                }
                let line: String = self.chars[start..self.position].iter().collect();
                let ended = self.peek(0).is_some();
                if ended {
                    self.position += 1;
                }
                let line = if strip_tabs {
                    line.trim_start_matches('\t')
                } else {
                    &line
                };
                if line == delimiter {
                    break;
                }
                if !ended {
                    return None;
                }
                body.push_str(line);
                body.push('\n');
            }
            self.tokens[index] = Token::Redirect(Redirect::Input { body });
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn words(script: &Script) -> Vec<Vec<Vec<&str>>> {
        script
            .iter()
            .map(|pipeline| {
                pipeline
                    .iter()
                    .map(|simple| simple.words.iter().map(String::as_str).collect())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn splits_lists_pipelines_and_quotes() {
        let script = parse(
            r#"ls -la && which cargo || echo "no \"cargo\""; rg -n 'fn main' src | head -20"#,
        )
        .unwrap();
        assert_eq!(
            words(&script),
            vec![
                vec![vec!["ls", "-la"]],
                vec![vec!["which", "cargo"]],
                vec![vec!["echo", "no \"cargo\""]],
                vec![vec!["rg", "-n", "fn main", "src"], vec!["head", "-20"]],
            ]
        );
    }

    #[test]
    fn reads_redirections() {
        let script =
            parse("cargo init --name snake . 2>&1 | head -20 > out.txt 2>/dev/null").unwrap();
        assert_eq!(script[0][0].redirects, vec![Redirect::Duplicate]);
        assert_eq!(
            script[0][1].redirects,
            vec![
                Redirect::Write {
                    target: "out.txt".into()
                },
                Redirect::Write {
                    target: "/dev/null".into()
                }
            ]
        );
        assert_eq!(script[0][1].words, ["head", "-20"]);
    }

    #[test]
    fn reads_heredocs() {
        let script = parse(
            "cat > src/main.rs <<'EOF'\nfn main() {\n    println!(\"$x\");\n}\nEOF\ncargo build",
        )
        .unwrap();
        assert_eq!(script.len(), 2);
        assert_eq!(
            script[0][0].redirects,
            vec![
                Redirect::Write {
                    target: "src/main.rs".into()
                },
                Redirect::Input {
                    body: "fn main() {\n    println!(\"$x\");\n}\n".into()
                }
            ]
        );
        assert_eq!(script[1][0].words, ["cargo", "build"]);
    }

    #[test]
    fn refuses_what_it_does_not_understand() {
        for source in [
            "echo $(date)",
            "echo `date`",
            "(cd x && ls)",
            "diff <(ls a) <(ls b)",
            "echo 'unterminated",
            "cat <<EOF\nno end",
            "ls |",
            "a && ",
            "case x in a) ;; esac",
        ] {
            assert_eq!(parse(source), None, "{source:?}");
        }
        assert!(parse("ls # a comment\npwd").is_some());
        assert!(parse("echo a \\\n  b").is_some());
        assert!(parse("cargo build &&\n  cargo test").is_some());
    }

    #[test]
    fn every_prefix_is_safe_to_parse() {
        let full = "cat > a.txt <<'EOF'\nhi\nEOF\nls -la | grep \"x y\" && echo done";
        for end in full.char_indices().map(|(i, _)| i) {
            let _ = parse(&full[..end]);
        }
        assert_eq!(parse("cat > a.txt <<'EOF'\nhi\n"), None);
    }
}
