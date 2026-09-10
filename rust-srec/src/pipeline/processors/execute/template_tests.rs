//! These tests inspect strings and bindings only. They must never spawn a shell,
//! helper process, or external parser, including for malformed inputs.

use super::*;

fn prepare(
    shell: ShellKind,
    source: &str,
    input: &str,
    output: &str,
) -> Result<PreparedShellCommand, TemplateError> {
    compile(
        source,
        shell,
        |name| match name {
            "input" => Some(input),
            "output" => Some(output),
            _ => None,
        },
        "_TEST",
    )
}

#[test]
fn trusted_templates_without_known_placeholders_are_unchanged() {
    for shell in [ShellKind::Posix, ShellKind::Cmd] {
        for source in [
            "",
            "echo {unknown}",
            "if true; then echo '$HOME'; fi",
            "@call fixed.cmd | more",
            "cat <<'END'\nfixed\nEND",
            "tool{one,two} fixed",
        ] {
            let prepared = prepare(shell, source, "value", "out").unwrap();
            assert_eq!(prepared.command, source);
            assert!(prepared.environment.is_empty());
            assert!(!prepared.protected_cmd);
        }
    }
}

#[test]
fn posix_preserves_supported_operators_and_uses_bindings_in_each_quote_context() {
    let prepared = prepare(
        ShellKind::Posix,
        "tool {input} 'pre{input}post' \"{input}\" | other > {output} && third || fourth; last",
        "a 'quoted' $value\nline",
        "output path",
    )
    .unwrap();
    assert_eq!(
        prepared.command,
        "tool \"${_TEST_0}\" 'pre'\"${_TEST_1}\"'post' \"${_TEST_2}\" | other > \"${_TEST_3}\" && third || fourth; last"
    );
    assert_eq!(
        prepared
            .environment
            .iter()
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>(),
        [
            "a 'quoted' $value\nline",
            "a 'quoted' $value\nline",
            "a 'quoted' $value\nline",
            "output path"
        ]
    );
}

#[test]
fn posix_comments_and_escaped_placeholders_do_not_change_following_context() {
    let prepared = prepare(
        ShellKind::Posix,
        "tool \\{input} # ' \" {input}\nnext {input} {unknown}",
        "one",
        "two",
    )
    .unwrap();
    assert_eq!(
        prepared.command,
        "tool \\{input} # ' \" {input}\nnext \"${_TEST_0}\" {unknown}"
    );
    assert_eq!(prepared.environment, [("_TEST_0".into(), "one".into())]);
}

#[test]
fn posix_rejects_contexts_outside_the_supported_grammar() {
    for source in [
        "{input} arg",
        "tool \"$HOME\" {input}",
        "tool ${name:-{input}}",
        "tool $(({input}+1))",
        "tool $(other {input})",
        "tool `other {input}`",
        "cat <<END\nE{input}\nEND",
        "cat <<\"END MARK\"\n{input}\nEND MARK",
        "(tool {input})",
        "tool {input} & other",
        "eval '{input}'",
        "env sh -c '{input}'",
        "! sh -c '{input}'",
        "A=value tool {input}",
        "tool '{input}",
        "tool {input} >",
        "tool {input} > && other",
        "tool {input} 2>&1{output}",
        "tool {input} && ; other",
        "tool {input} |",
        "tool {input} \\\nother",
    ] {
        let error = prepare(ShellKind::Posix, source, "ND", "other").unwrap_err();
        assert!(error.to_string().contains("byte"), "{source}");
        assert!(error.to_string().contains("program/args"), "{source}");
    }
}

#[test]
fn descriptor_duplication_and_file_redirects_keep_distinct_roles() {
    let prepared = prepare(
        ShellKind::Posix,
        "2> {output} tool {input} 2>&1 < fixed",
        "in",
        "out",
    )
    .unwrap();
    assert_eq!(
        prepared.command,
        "2> \"${_TEST_0}\" tool \"${_TEST_1}\" 2>&1 < fixed"
    );
    for shell in [ShellKind::Posix, ShellKind::Cmd] {
        assert!(prepare(shell, "tool {input} > {output}", "in", "").is_err());
    }
}

#[test]
fn posix_command_names_reject_brace_expansion_but_argument_tokens_stay_literal() {
    for source in ["tool{one,two} {input}", "tool{1..2} {input}"] {
        let error = prepare(ShellKind::Posix, source, "media.wav", "out").unwrap_err();
        assert!(error.to_string().contains("command names must be fixed"));
    }
    let prepared = prepare(
        ShellKind::Posix,
        "tool {unknown} {input9} {input}",
        "media.wav",
        "out",
    )
    .unwrap();
    assert_eq!(prepared.command, "tool {unknown} {input9} \"${_TEST_0}\"");
    assert_eq!(
        prepared.environment,
        [("_TEST_0".into(), "media.wav".into())]
    );
}

#[test]
fn windows_classifies_prefixed_and_escaped_command_names_before_binding() {
    for program in [
        "@call",
        "@@CALL",
        "c^a^l^l",
        "\"call\"",
        "@\"cmd.exe\"",
        "set",
        "echo",
        "tool.cmd",
        "tool.bat",
        "powershell.exe",
        "C:\\Windows\\System32\\cmd.exe",
    ] {
        assert!(
            prepare(
                ShellKind::Cmd,
                &format!("{program} {{input}}"),
                "data",
                "out"
            )
            .is_err(),
            "{program}"
        );
    }
    assert!(
        prepare(
            ShellKind::Cmd,
            "@tool.exe {input} && other.com {output}",
            "data",
            "out"
        )
        .is_ok()
    );
    assert!(
        prepare(
            ShellKind::Cmd,
            "@echo fixed && tool.exe {input}",
            "data",
            "out"
        )
        .is_ok()
    );
}

#[test]
fn windows_rejects_reparsing_and_ambiguous_literal_escapes() {
    assert!(
        prepare(
            ShellKind::Cmd,
            "tool.exe \"fixed\nline\" {input}",
            "data",
            "out"
        )
        .is_err()
    );
    for source in [
        "tool.exe {input} | other.exe",
        "tool.exe %PATH% {input}",
        "tool.exe !NAME! {input}",
        "tool.exe \"literal!\" {input}",
        "tool.exe \"literal^caret\" {input}",
        "tool.exe {input}\nother.exe",
        "tool.exe ({input})",
        "tool.exe \"prefix\\{input}\\\"",
        "tool.exe \"pre\"\"{input}\"",
        "tool.exe pre^&{input}",
    ] {
        assert!(
            prepare(ShellKind::Cmd, source, "data", "out").is_err(),
            "{source}"
        );
    }
    for input in ["a\nb", "a\rb", "a\tb", "a\0b"] {
        assert!(prepare(ShellKind::Cmd, "tool.exe {input}", input, "out").is_err());
    }
}

#[test]
fn windows_fixed_native_paths_allow_short_names_and_literal_brackets() {
    for source in [
        r#""C:\Users\RUNNER~1\AppData\Local\Temp\capture.exe" {input}"#,
        r#""C:\media [mix]\capture.exe" {input}"#,
    ] {
        let prepared = prepare(ShellKind::Cmd, source, "media.wav", "out").unwrap();
        assert!(
            prepared
                .command
                .starts_with(source.split(" {input}").next().unwrap())
        );
        assert_eq!(
            prepared.environment,
            [("_TEST_0".into(), "media.wav".into())]
        );
    }
    for source in [
        r#""C:\media\capture?.exe" {input}"#,
        r#""C:\media\*.exe" {input}"#,
        r#""\\?\C:\media\capture.exe" {input}"#,
    ] {
        assert!(prepare(ShellKind::Cmd, source, "media.wav", "out").is_err());
    }
    for source in ["~/capture {input}", "capture[12] {input}"] {
        assert!(prepare(ShellKind::Posix, source, "media.wav", "out").is_err());
    }
}

#[test]
fn windows_size_guard_counts_encoded_binding_units() {
    for value in ["a".repeat(8192), "\\".repeat(4096), "🎵".repeat(4096)] {
        let error = prepare(ShellKind::Cmd, "tool.exe {input}", &value, "out").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("binding exceeds"), "{message}");
        assert!(message.contains("program/args"));
        assert!(message.contains("file data interface"));
    }
}

#[test]
fn windows_size_guard_bounds_source_even_when_bindings_shrink() {
    // 8,159 units remain after the 32-unit launcher reserve. A private reference
    // is longer than this one-character value, so source is the limiting form.
    let boundary = format!("tool.exe {} {{input}}", "a".repeat(8138));
    let prepared = prepare(ShellKind::Cmd, &boundary, "x", "out").unwrap();
    assert_eq!(prepared.command.encode_utf16().count(), 8159);
    let oversized = format!("tool.exe {} {{input}}", "a".repeat(8139));
    let error = prepare(ShellKind::Cmd, &oversized, "x", "out").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("generated Windows command exceeds")
    );

    // Production-generated names are longer than the short test prefix too.
    let error = compile(
        "tool.exe {input}",
        ShellKind::Cmd,
        |name| (name == "input").then_some("x"),
        &format!("_{}", "A".repeat(8144)),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("generated Windows command exceeds")
    );
}

#[test]
fn windows_size_guard_bounds_combined_expansion_and_surrogate_pairs() {
    // The fixed program, space and surrounding quotes occupy 11 UTF-16 units.
    for value in ["a".repeat(8148), "🎵".repeat(4074)] {
        assert!(prepare(ShellKind::Cmd, "tool.exe {input}", &value, "out").is_ok());
    }
    for value in ["a".repeat(8149), "🎵".repeat(4075)] {
        let error = prepare(ShellKind::Cmd, "tool.exe {input}", &value, "out").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("expanded Windows command exceeds")
        );
    }
    // Each repeated placeholder gets its own small binding; their total is what
    // exceeds cmd's limit. A per-value-only check would accept this command.
    assert!(
        prepare(
            ShellKind::Cmd,
            "tool.exe {input} {input}",
            &"a".repeat(4072),
            "out",
        )
        .is_ok()
    );
    let error = prepare(
        ShellKind::Cmd,
        "tool.exe {input} {input}",
        &"a".repeat(4073),
        "out",
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("expanded Windows command exceeds")
    );

    let value = "a".repeat(8192);
    assert!(prepare(ShellKind::Posix, "tool {input}", &value, "out").is_ok());
    let source = format!("tool.exe {value}");
    let raw = prepare(ShellKind::Cmd, &source, "unused", "out").unwrap();
    assert_eq!(raw.command, source);
    assert!(!raw.protected_cmd);
}

// Independent model of the published Microsoft C argument-decoding rules.
// This checks encoding mathematics; it does not model cmd or certify native IO.
fn decode_c_arguments(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut arguments = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && matches!(chars[i], ' ' | '\t') {
            i += 1;
        }
        if i == chars.len() {
            break;
        }
        let mut value = String::new();
        let mut quoted = false;
        loop {
            if i == chars.len() || (!quoted && matches!(chars[i], ' ' | '\t')) {
                break;
            }
            let start = i;
            while i < chars.len() && chars[i] == '\\' {
                i += 1;
            }
            let count = i - start;
            if i < chars.len() && chars[i] == '"' {
                value.extend(std::iter::repeat_n('\\', count / 2));
                if count % 2 == 1 {
                    value.push('"');
                    i += 1;
                } else if quoted && chars.get(i + 1) == Some(&'"') {
                    value.push('"');
                    i += 2;
                } else {
                    quoted = !quoted;
                    i += 1;
                }
            } else {
                value.extend(std::iter::repeat_n('\\', count));
                if i < chars.len() {
                    value.push(chars[i]);
                    i += 1;
                }
            }
        }
        arguments.push(value);
    }
    arguments
}

#[test]
fn windows_encodes_the_complete_word_across_literal_and_value_boundaries() {
    let alphabet = ["", "a", " ", "\\", "\"", "λ", "%PATH%", "!value!", "&|<>"];
    for left in alphabet {
        for right in alphabet {
            let prepared = prepare(
                ShellKind::Cmd,
                "tool.exe pre\\{input}{output}\\tail",
                left,
                right,
            )
            .unwrap();
            assert_eq!(prepared.command, "tool.exe \"!_TEST_0!\"");
            assert_eq!(prepared.environment.len(), 1);
            assert_eq!(
                decode_c_arguments(&format!("\"{}\"", prepared.environment[0].1)),
                [format!("pre\\{left}{right}\\tail")]
            );
        }
    }
}

#[test]
fn windows_quotes_and_trailing_backslashes_round_trip_in_the_c_argument_model() {
    for value in [
        "one",
        "a b",
        "\\",
        "a\\\\",
        "a\"b",
        "a\\\"b",
        "\"\\",
        "λ%PATH%!value!&",
    ] {
        let prepared = prepare(ShellKind::Cmd, "tool.exe \"{input}\"", value, "out").unwrap();
        assert_eq!(
            decode_c_arguments(&format!("\"{}\"", prepared.environment[0].1)),
            [value]
        );
        assert!(!prepared.command.contains(value));
        assert!(prepared.protected_cmd);
    }
}

#[test]
fn windows_redirect_paths_do_not_use_native_argument_backslash_encoding() {
    let prepared = prepare(
        ShellKind::Cmd,
        "tool.exe {input} > {output} && next.exe fixed",
        "path\\",
        "path\\",
    )
    .unwrap();
    assert_eq!(prepared.environment[0].1, "path\\\\");
    assert_eq!(prepared.environment[1].1, "path\\");
    assert!(
        prepare(
            ShellKind::Cmd,
            "tool.exe fixed > {output}",
            "in",
            "path\"quote"
        )
        .is_err()
    );
}

#[test]
fn windows_json_uses_backslash_quotes_and_private_delayed_bindings() {
    let value = r#"["C:\\media files\\Recording 01.wav","C:\\media files\\Encore!.wav"]"#;
    let prepared = prepare(ShellKind::Cmd, "tool.exe {input}", value, "out").unwrap();
    assert_eq!(prepared.command, "tool.exe \"!_TEST_0!\"");
    assert_eq!(
        prepared.environment[0].1,
        r#"[\"C:\\media files\\Recording 01.wav\",\"C:\\media files\\Encore!.wav\"]"#
    );
    assert_eq!(
        decode_c_arguments(&format!("\"{}\"", prepared.environment[0].1)),
        [value]
    );
    let punctuation = prepare(
        ShellKind::Cmd,
        "tool.exe {input} > {output}",
        "Encore! 50%.wav",
        "Copy! 50%.wav",
    )
    .unwrap();
    assert_eq!(punctuation.environment[0].1, "Encore! 50%.wav");
    assert_eq!(punctuation.environment[1].1, "Copy! 50%.wav");
    let trusted = prepare(
        ShellKind::Cmd,
        "echo literal! ^& %NAME%",
        "unused",
        "unused",
    )
    .unwrap();
    assert_eq!(trusted.command, "echo literal! ^& %NAME%");
    assert!(!trusted.protected_cmd);
}

#[test]
fn empty_bare_values_disappear_but_existing_quotes_keep_an_empty_argument() {
    for (shell, source, expected) in [
        (
            ShellKind::Posix,
            "tool {input} '{input}' \"{input}\"",
            "tool  '' \"\"",
        ),
        (
            ShellKind::Cmd,
            "tool.exe {input} \"{input}\"",
            "tool.exe  \"\"",
        ),
    ] {
        let prepared = prepare(shell, source, "", "").unwrap();
        assert_eq!(prepared.command, expected);
        assert!(prepared.environment.is_empty());
    }
    let prepared = prepare(ShellKind::Posix, "tool {input}#label && next", "", "").unwrap();
    assert_eq!(prepared.command, "tool \"\"#label && next");
}

#[test]
fn inserted_text_never_changes_compiled_command_structure_or_expands_again() {
    for shell in [ShellKind::Posix, ShellKind::Cmd] {
        let source = "tool.exe {input} && other.exe {output}";
        let simple = prepare(shell, source, "first", "second").unwrap();
        let special = prepare(
            shell,
            source,
            "{output} %Y %PATH% !name! $name",
            "quotes'\"&|;(){}",
        )
        .unwrap();
        assert_eq!(simple.command, special.command);
        assert_eq!(simple.environment.len(), special.environment.len());
    }
    assert!(prepare(ShellKind::Posix, "tool {input}", "with\0nul", "out").is_err());
}
