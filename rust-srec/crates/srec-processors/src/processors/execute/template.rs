//! Pure compilation of the supported value-bearing command-template grammar.
//! This module never starts a process or evaluates shell source.

use std::fmt;

#[path = "windows.rs"]
mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellKind {
    Posix,
    Cmd,
}

#[derive(Debug)]
pub(crate) struct PreparedShellCommand {
    pub command: String,
    pub environment: Vec<(String, String)>,
    pub protected_cmd: bool,
}

#[derive(Debug)]
pub(crate) struct TemplateError {
    offset: usize,
    reason: &'static str,
}

impl TemplateError {
    fn unsupported(offset: usize, reason: &'static str) -> Self {
        Self { offset, reason }
    }
}

impl fmt::Display for TemplateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Unsupported execute template at byte {}: {}. Use program/args for literal arguments or a fixed script with an explicit data interface",
            self.offset, self.reason
        )
    }
}
impl std::error::Error for TemplateError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quote {
    Bare,
    Single,
    Double,
}

#[derive(Debug)]
struct Binding {
    start: usize,
    end: usize,
    quote: Quote,
    value: String,
}

#[derive(Debug)]
struct Word {
    start: usize,
    end: usize,
    text: String,
    bindings: Vec<Binding>,
    quoted: bool,
    ambiguous_windows_quote: bool,
}

#[derive(Debug, Clone, Copy)]
enum Operator {
    Sequence,
    And,
    Or,
    Pipe,
    Redirect,
    DuplicateDescriptor,
}

#[derive(Debug)]
enum Token {
    Word(Word),
    Operator(Operator, usize),
}

fn resolved<'a>(
    source: &str,
    start: usize,
    resolve: &impl Fn(&str) -> Option<&'a str>,
) -> Option<(usize, &'a str)> {
    let tail = source.get(start..)?.strip_prefix('{')?;
    let end = tail.find('}')?;
    Some((start + end + 2, resolve(&tail[..end])?))
}

fn word<'a>(
    source: &str,
    start: usize,
    shell: ShellKind,
    resolve: &impl Fn(&str) -> Option<&'a str>,
) -> Result<Word, TemplateError> {
    let mut i = start;
    let mut quote = Quote::Bare;
    let mut text = String::new();
    let mut bindings = Vec::new();
    let mut quoted = false;
    let mut ambiguous_windows_quote = false;
    while let Some(character) = source[i..].chars().next() {
        if quote == Quote::Bare
            && matches!(
                character,
                ' ' | '\t' | '\n' | '\r' | ';' | '&' | '|' | '<' | '>'
            )
        {
            break;
        }
        if character == '\0' {
            return Err(TemplateError::unsupported(
                i,
                "NUL is not a process argument",
            ));
        }
        if character == '{'
            && let Some((end, value)) = resolved(source, i, resolve)
        {
            if value.contains('\0') {
                return Err(TemplateError::unsupported(i, "a placeholder contains NUL"));
            }
            bindings.push(Binding {
                start: i,
                end,
                quote,
                value: value.into(),
            });
            text.push_str(value);
            i = end;
            continue;
        }
        if character == '"' && quote != Quote::Single {
            if shell == ShellKind::Cmd
                && (source[..i].ends_with('\\')
                    || (quote == Quote::Double && source.as_bytes().get(i + 1) == Some(&b'"')))
            {
                ambiguous_windows_quote = true;
            }
            quote = if quote == Quote::Double {
                Quote::Bare
            } else {
                Quote::Double
            };
            quoted = true;
            i += 1;
            continue;
        }
        if character == '\'' && shell == ShellKind::Posix && quote != Quote::Double {
            quote = if quote == Quote::Single {
                Quote::Bare
            } else {
                Quote::Single
            };
            quoted = true;
            i += 1;
            continue;
        }
        if (character == '\\' && shell == ShellKind::Posix && quote != Quote::Single)
            || (character == '^' && shell == ShellKind::Cmd && quote == Quote::Bare)
        {
            if shell == ShellKind::Cmd {
                ambiguous_windows_quote = true;
            }
            let Some(next) = source[i + 1..].chars().next() else {
                return Err(TemplateError::unsupported(
                    i,
                    "an escape has no following character",
                ));
            };
            if matches!(next, '\n' | '\r') {
                return Err(TemplateError::unsupported(
                    i,
                    "line continuation is outside the supported grammar",
                ));
            }
            if shell == ShellKind::Posix
                && quote == Quote::Double
                && !matches!(next, '$' | '`' | '"' | '\\')
            {
                text.push(character);
                i += 1;
            } else {
                text.push(next);
                i += 1 + next.len_utf8();
            }
            continue;
        }
        if shell == ShellKind::Posix && quote != Quote::Single && matches!(character, '$' | '`') {
            return Err(TemplateError::unsupported(
                i,
                "shell expansions and substitutions cannot be mixed with placeholders",
            ));
        }
        if quote == Quote::Bare && matches!(character, '(' | ')') {
            return Err(TemplateError::unsupported(
                i,
                "command groups require a fixed script",
            ));
        }
        text.push(character);
        i += character.len_utf8();
    }
    if quote != Quote::Bare {
        return Err(TemplateError::unsupported(start, "quotes must be balanced"));
    }
    Ok(Word {
        start,
        end: i,
        text,
        bindings,
        quoted,
        ambiguous_windows_quote,
    })
}

fn lex<'a>(
    source: &str,
    shell: ShellKind,
    resolve: &impl Fn(&str) -> Option<&'a str>,
) -> Result<Vec<Token>, TemplateError> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while let Some(character) = source[i..].chars().next() {
        match character {
            ' ' | '\t' => i += 1,
            '#' if shell == ShellKind::Posix => {
                // Quotes and placeholders inside a comment cannot affect later words.
                i += source[i..].find('\n').unwrap_or(source.len() - i);
            }
            '\r' | '\n' | ';' | '&' | '|' => {
                let offset = i;
                let doubled = source.as_bytes().get(i + 1) == Some(&(character as u8));
                let operator = match (character, doubled) {
                    ('&', true) => Operator::And,
                    ('|', true) => Operator::Or,
                    ('|', false) if shell == ShellKind::Posix => Operator::Pipe,
                    ('|', false) => {
                        return Err(TemplateError::unsupported(
                            i,
                            "Windows pipes with placeholders are not supported",
                        ));
                    }
                    ('&', false) if shell == ShellKind::Cmd => Operator::Sequence,
                    ('&', false) => {
                        return Err(TemplateError::unsupported(
                            i,
                            "background commands with placeholders are not supported",
                        ));
                    }
                    (';', _) if shell == ShellKind::Cmd => {
                        return Err(TemplateError::unsupported(
                            i,
                            "use & between Windows commands",
                        ));
                    }
                    (';', true) => {
                        return Err(TemplateError::unsupported(
                            i,
                            "case statements require a fixed script",
                        ));
                    }
                    ('\r', _) if shell == ShellKind::Posix => {
                        return Err(TemplateError::unsupported(
                            i,
                            "use LF newlines in POSIX placeholder templates",
                        ));
                    }
                    ('\r' | '\n', _) if shell == ShellKind::Cmd => {
                        return Err(TemplateError::unsupported(
                            i,
                            "multiline Windows templates with placeholders require a fixed script",
                        ));
                    }
                    _ => Operator::Sequence,
                };
                i += if doubled { 2 } else { 1 };
                tokens.push(Token::Operator(operator, offset));
            }
            '<' | '>' => {
                let offset = i;
                i += 1;
                if source.as_bytes().get(i) == Some(&b'<') {
                    return Err(TemplateError::unsupported(
                        offset,
                        "here-documents and here-strings with placeholders require an explicit data interface",
                    ));
                }
                if character == '>' && source.as_bytes().get(i) == Some(&b'>') {
                    i += 1;
                }
                let operator = if source.as_bytes().get(i) == Some(&b'&') {
                    i += 1;
                    let descriptor = i;
                    while source.as_bytes().get(i).is_some_and(u8::is_ascii_digit) {
                        i += 1;
                    }
                    if descriptor == i {
                        return Err(TemplateError::unsupported(
                            offset,
                            "descriptor duplication requires literal digits",
                        ));
                    }
                    if source[i..].chars().next().is_some_and(|c| {
                        !matches!(c, ' ' | '\t' | '\r' | '\n' | ';' | '&' | '|' | '<' | '>')
                    }) {
                        return Err(TemplateError::unsupported(
                            offset,
                            "descriptor duplication cannot contain placeholder or word suffixes",
                        ));
                    }
                    Operator::DuplicateDescriptor
                } else {
                    Operator::Redirect
                };
                tokens.push(Token::Operator(operator, offset));
            }
            _ => {
                let next = word(source, i, shell, resolve)?;
                i = next.end;
                tokens.push(Token::Word(next));
            }
        }
    }
    Ok(tokens)
}

fn validate_program(
    word: &Word,
    shell: ShellKind,
    stage_bindings: bool,
) -> Result<(), TemplateError> {
    if !word.bindings.is_empty() {
        return Err(TemplateError::unsupported(
            word.start,
            "the command name must be fixed; placeholders belong in arguments or file redirects",
        ));
    }
    let program = if shell == ShellKind::Cmd {
        word.text.trim_start_matches('@')
    } else {
        &word.text
    };
    if shell == ShellKind::Cmd && program.starts_with(r"\\?\") {
        return Err(TemplateError::unsupported(
            word.start,
            "extended Windows namespace executable paths require shell-specific resolution outside the supported grammar",
        ));
    }
    if program.is_empty()
        || program.contains(['=', '*', '?'])
        || (shell == ShellKind::Posix && program.contains(['[', ']', '~', '{', '}']))
    {
        return Err(TemplateError::unsupported(
            word.start,
            "command names must be fixed; assignment prefixes and command-name expansion are not supported",
        ));
    }
    let basename = program.rsplit(['/', '\\']).next().unwrap_or("");
    if shell == ShellKind::Cmd {
        return windows::validate_program(basename, word.start, stage_bindings);
    }
    if matches!(
        basename,
        "!" | "eval"
            | "exec"
            | "source"
            | "."
            | "trap"
            | "alias"
            | "unalias"
            | "set"
            | "export"
            | "readonly"
            | "unset"
            | "read"
            | "return"
            | "break"
            | "continue"
            | "exit"
            | "sh"
            | "bash"
            | "dash"
            | "ash"
            | "zsh"
            | "ksh"
            | "env"
            | "command"
            | "builtin"
            | "if"
            | "then"
            | "else"
            | "elif"
            | "fi"
            | "for"
            | "while"
            | "until"
            | "do"
            | "done"
            | "case"
            | "esac"
            | "select"
            | "function"
            | "time"
            | "coproc"
            | "{"
            | "}"
    ) {
        return Err(TemplateError::unsupported(
            word.start,
            "shell wrappers and compound syntax require a fixed script with an explicit data interface",
        ));
    }
    Ok(())
}

fn bind(environment: &mut Vec<(String, String)>, prefix: &str, value: String) -> String {
    let name = format!("{prefix}_{}", environment.len());
    environment.push((name.clone(), value));
    name
}

/// `prefix` is a process-local, generated identifier, never configuration data.
pub(crate) fn compile<'a>(
    source: &str,
    shell: ShellKind,
    resolve: impl Fn(&str) -> Option<&'a str>,
    prefix: &str,
) -> Result<PreparedShellCommand, TemplateError> {
    let has_values = source
        .match_indices('{')
        .any(|(start, _)| resolved(source, start, &resolve).is_some());
    if !has_values {
        return Ok(PreparedShellCommand {
            command: source.into(),
            environment: Vec::new(),
            protected_cmd: false,
        });
    }
    if !prefix.starts_with('_')
        || !prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(TemplateError::unsupported(
            0,
            "invalid internal binding identifier",
        ));
    }
    if shell == ShellKind::Cmd && source.contains('%') {
        return Err(TemplateError::unsupported(
            0,
            "percent-based shell expansion cannot be mixed with Windows placeholders",
        ));
    }
    if shell == ShellKind::Cmd
        && let Some(offset) = source.find(['!', '^'])
    {
        return Err(TemplateError::unsupported(
            offset,
            "literal exclamation marks and carets cannot be mixed with Windows placeholders; put those characters in argument values",
        ));
    }
    if shell == ShellKind::Cmd
        && let Some((offset, _)) = source
            .char_indices()
            .find(|(_, character)| character.is_control() && *character != '\t')
    {
        return Err(TemplateError::unsupported(
            offset,
            "Windows placeholder templates must be a single line without control characters",
        ));
    }
    let tokens = lex(source, shell, &resolve)?;
    let mut environment = Vec::new();
    let mut command = String::with_capacity(source.len());
    let mut copied = 0;
    let mut program = false;
    let mut redirect = false;
    let mut has_stage = false;
    let mut required_stage = false;
    let mut stage_start = 0;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            Token::Operator(operator, offset) => {
                if redirect {
                    return Err(TemplateError::unsupported(
                        *offset,
                        "a file redirect requires one target word",
                    ));
                }
                match operator {
                    Operator::Redirect => {
                        redirect = true;
                        has_stage = true;
                    }
                    Operator::DuplicateDescriptor => has_stage = true,
                    Operator::Sequence => {
                        if required_stage {
                            return Err(TemplateError::unsupported(
                                *offset,
                                "a conditional or pipe requires a following command",
                            ));
                        }
                        program = false;
                        has_stage = false;
                        stage_start = index + 1;
                    }
                    Operator::And | Operator::Or | Operator::Pipe => {
                        if !has_stage || index + 1 == tokens.len() {
                            return Err(TemplateError::unsupported(
                                *offset,
                                "a conditional or pipe requires commands on both sides",
                            ));
                        }
                        program = false;
                        has_stage = false;
                        required_stage = true;
                        stage_start = index + 1;
                    }
                }
            }
            Token::Word(word) => {
                let descriptor = word.bindings.is_empty()
                    && !word.quoted
                    && !word.text.is_empty()
                    && word.text.bytes().all(|byte| byte.is_ascii_digit())
                    && matches!(tokens.get(index + 1), Some(Token::Operator(Operator::Redirect | Operator::DuplicateDescriptor, offset)) if *offset == word.end);
                if descriptor {
                    continue;
                }
                let target = redirect;
                redirect = false;
                if !target && !program {
                    let stage_bindings = tokens[stage_start..]
                        .iter()
                        .take_while(|token| {
                            !matches!(
                                token,
                                Token::Operator(
                                    Operator::Sequence
                                        | Operator::And
                                        | Operator::Or
                                        | Operator::Pipe,
                                    _
                                )
                            )
                        })
                        .any(
                            |token| matches!(token, Token::Word(word) if !word.bindings.is_empty()),
                        );
                    validate_program(word, shell, stage_bindings)?;
                    program = true;
                }
                has_stage = true;
                required_stage = false;
                if word.bindings.is_empty() {
                    continue;
                }
                if target && word.text.is_empty() {
                    return Err(TemplateError::unsupported(
                        word.start,
                        "a placeholder produced an empty redirect path",
                    ));
                }
                if shell == ShellKind::Cmd {
                    if word.ambiguous_windows_quote {
                        return Err(TemplateError::unsupported(
                            word.start,
                            "caret escapes or literal quote/backslash combinations in a placeholder word are ambiguous; put the complete argument in args",
                        ));
                    }
                    let value = windows::encode_word(&word.text, target, word.start)?;
                    command.push_str(&source[copied..word.start]);
                    if word.text.is_empty() && word.quoted {
                        command.push_str("\"\"");
                    } else if !word.text.is_empty() {
                        let name = bind(&mut environment, prefix, value);
                        // Only generated names participate in delayed expansion.
                        // Values (including ! and %) are not source tokens.
                        command.push_str(&format!("\"!{name}!\""));
                    }
                    copied = word.end;
                } else {
                    for binding in &word.bindings {
                        command.push_str(&source[copied..binding.start]);
                        if !binding.value.is_empty() {
                            let name = bind(&mut environment, prefix, binding.value.clone());
                            match binding.quote {
                                Quote::Bare => command.push_str(&format!("\"${{{name}}}\"")),
                                Quote::Double => command.push_str(&format!("${{{name}}}")),
                                Quote::Single => command.push_str(&format!("'\"${{{name}}}\"'")),
                            }
                        } else if binding.quote == Quote::Bare
                            && (!word.text.is_empty() || word.quoted)
                        {
                            // Keep an existing word boundary when an empty
                            // fragment precedes literal '#' or similar syntax.
                            command.push_str("\"\"");
                        }
                        copied = binding.end;
                    }
                }
            }
        }
    }
    if redirect {
        return Err(TemplateError::unsupported(
            source.len(),
            "a file redirect requires one target word",
        ));
    }
    if required_stage {
        return Err(TemplateError::unsupported(
            source.len(),
            "a conditional or pipe requires a following command",
        ));
    }
    command.push_str(&source[copied..]);
    if shell == ShellKind::Cmd {
        windows::validate_size(&command, &environment)?;
    }
    Ok(PreparedShellCommand {
        command,
        environment,
        protected_cmd: shell == ShellKind::Cmd,
    })
}

#[cfg(test)]
#[path = "template_tests.rs"]
mod tests;
