//! Windows restrictions and complete-word encoding. No native process calls.

use super::TemplateError;

const CMD_UTF16_LIMIT: usize = 8191;
// Covers the fixed cmd.exe name, /D /V:ON /S /C, outer quotes and terminator.
const COMMAND_WRAPPER_UNITS: usize = 32;

fn bounded_utf16_len(value: &str) -> usize {
    value.encode_utf16().take(CMD_UTF16_LIMIT + 1).count()
}

pub(super) fn validate_size(
    command: &str,
    environment: &[(String, String)],
) -> Result<(), TemplateError> {
    // cmd ignores overlong inherited variables instead of reporting an error.
    // Include the name, '=' and terminator to remain below that boundary.
    for (name, value) in environment {
        if bounded_utf16_len(name)
            .saturating_add(bounded_utf16_len(value))
            .saturating_add(2)
            > CMD_UTF16_LIMIT
        {
            return Err(TemplateError::unsupported(
                0,
                "an encoded Windows placeholder binding exceeds cmd's 8,191 UTF-16-unit limit; use a file data interface for large values",
            ));
        }
    }
    let command_limit = CMD_UTF16_LIMIT - COMMAND_WRAPPER_UNITS;
    let source_units = bounded_utf16_len(command);
    if source_units > command_limit {
        return Err(TemplateError::unsupported(
            0,
            "the generated Windows command exceeds cmd's 8,191 UTF-16-unit limit including the reserved launcher overhead; use a file data interface for large values",
        ));
    }
    // Source cannot contain literal '!'. Count every generated reference and
    // allow no credit for shrinking words: this also bounds partial expansion.
    let mut expanded_units = source_units;
    for (name, value) in environment {
        let reference = format!("!{name}!");
        let growth = bounded_utf16_len(value).saturating_sub(bounded_utf16_len(&reference));
        expanded_units = expanded_units
            .saturating_add(growth.saturating_mul(command.matches(&reference).count()));
        if expanded_units > command_limit {
            return Err(TemplateError::unsupported(
                0,
                "the expanded Windows command exceeds cmd's 8,191 UTF-16-unit limit including the reserved launcher overhead; use a file data interface for large values",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_program(
    name: &str,
    offset: usize,
    stage_bindings: bool,
) -> Result<(), TemplateError> {
    let name = name.trim_end_matches(['.', ' ']).to_ascii_lowercase();
    // These literal-only stages neither reparse data nor change the binding
    // environment. Other builtins require their own grammar and data contract.
    if !stage_bindings && matches!(name.as_str(), "echo" | "ver") {
        return Ok(());
    }
    if matches!(
        name.as_str(),
        "assoc"
            | "break"
            | "call"
            | "cd"
            | "chdir"
            | "cls"
            | "color"
            | "copy"
            | "date"
            | "del"
            | "dir"
            | "echo"
            | "endlocal"
            | "erase"
            | "exit"
            | "for"
            | "ftype"
            | "goto"
            | "if"
            | "md"
            | "mkdir"
            | "mklink"
            | "move"
            | "path"
            | "pause"
            | "popd"
            | "prompt"
            | "pushd"
            | "rd"
            | "rem"
            | "ren"
            | "rename"
            | "rmdir"
            | "set"
            | "setlocal"
            | "shift"
            | "start"
            | "time"
            | "title"
            | "type"
            | "ver"
            | "verify"
            | "vol"
            | "cmd"
            | "cmd.exe"
            | "powershell"
            | "powershell.exe"
            | "pwsh"
            | "pwsh.exe"
            | "sh"
            | "sh.exe"
            | "bash"
            | "bash.exe"
    ) {
        return Err(TemplateError::unsupported(
            offset,
            "Windows placeholder commands support fixed native programs, not builtins or nested shells",
        ));
    }
    if name.is_empty() || name.contains(['!', '%', '@', ':']) {
        return Err(TemplateError::unsupported(
            offset,
            "the native executable name must be fixed and unambiguous",
        ));
    }
    if let Some((_, extension)) = name.rsplit_once('.')
        && !matches!(extension, "exe" | "com")
    {
        return Err(TemplateError::unsupported(
            offset,
            "Windows placeholder commands require native .exe/.com programs",
        ));
    }
    Ok(())
}

/// Encode a complete argument for the shared C/Shell32 backslash-quote rules.
/// The caller introduces it through delayed expansion after cmd parses syntax;
/// an ordinary percent expansion would expose these quotes to cmd's own parser.
/// Redirect paths use cmd's filename rules, not the child's argv decoder.
pub(super) fn encode_word(
    value: &str,
    redirect: bool,
    offset: usize,
) -> Result<String, TemplateError> {
    if value.chars().any(char::is_control) {
        return Err(TemplateError::unsupported(
            offset,
            "Windows placeholder words cannot contain control characters",
        ));
    }
    if redirect {
        if value.contains('"') {
            return Err(TemplateError::unsupported(
                offset,
                "Windows redirect paths cannot contain a double quote",
            ));
        }
        return Ok(value.into());
    }
    let mut encoded = String::with_capacity(value.len());
    let mut slashes = 0;
    for character in value.chars() {
        if character == '\\' {
            slashes += 1;
            continue;
        }
        encoded.extend(std::iter::repeat_n(
            '\\',
            if character == '"' {
                slashes * 2 + 1
            } else {
                slashes
            },
        ));
        slashes = 0;
        encoded.push(character);
    }
    encoded.extend(std::iter::repeat_n('\\', slashes * 2));
    Ok(encoded)
}
