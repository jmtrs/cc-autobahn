//! A tiny defensive scanner for a single string-valued property inside a
//! Codex Desktop tool-call argument blob.
//!
//! The blob is JavaScript source, not JSON: the escalation flag lives as
//! `sandbox_permissions: "require_escalated"` in a `tools.exec_command({…})`
//! call. It has to be recognized WITHOUT executing or fully parsing that
//! source, and without false-positiving on the same text appearing inside a
//! string literal, a template literal, or a comment. This module owns exactly
//! that: byte-level scanning that skips over JS quoted regions and comments,
//! then reads the JSON string value after a matched key. Nothing here touches
//! app state.

/// Finds `property` used as an object key at the JS top level (bare or quoted)
/// and returns its string value, skipping any occurrence inside a `'…'`/`"…"`/
/// `` `…` `` literal or a `//`/`/* */` comment. `None` when the property is
/// absent or its value isn't a plain string literal.
pub(super) fn extract_string_property(input: &str, property: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let property = property.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                let key_start = index;
                index += 1;
                let mut escaped = false;
                while index < bytes.len() {
                    if escaped {
                        escaped = false;
                    } else if bytes[index] == b'\\' {
                        escaped = true;
                    } else if bytes[index] == b'"' {
                        let key_end = index + 1;
                        if serde_json::from_slice::<String>(&bytes[key_start..key_end])
                            .ok()
                            .as_deref()
                            == std::str::from_utf8(property).ok()
                        {
                            if let Some(value) = string_value_after_colon(bytes, key_end) {
                                return Some(value);
                            }
                        }
                        index = key_end;
                        break;
                    }
                    index += 1;
                }
            }
            b'\'' | b'`' => {
                let quote = bytes[index];
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        index = (index + 2).min(bytes.len());
                    } else if bytes[index] == quote {
                        index += 1;
                        break;
                    } else {
                        index += 1;
                    }
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    index += 1;
                }
                index = (index + 2).min(bytes.len());
            }
            _ if bytes[index..].starts_with(property)
                && (index == 0 || !is_identifier_byte(bytes[index - 1]))
                && bytes
                    .get(index + property.len())
                    .is_none_or(|byte| !is_identifier_byte(*byte)) =>
            {
                let mut value_start = index + property.len();
                while bytes.get(value_start).is_some_and(u8::is_ascii_whitespace) {
                    value_start += 1;
                }
                if bytes.get(value_start) != Some(&b':') {
                    index += property.len();
                    continue;
                }
                value_start += 1;
                while bytes.get(value_start).is_some_and(u8::is_ascii_whitespace) {
                    value_start += 1;
                }
                if bytes.get(value_start) != Some(&b'"') {
                    return None;
                }
                let mut end = value_start + 1;
                let mut escaped = false;
                while end < bytes.len() {
                    if escaped {
                        escaped = false;
                    } else if bytes[end] == b'\\' {
                        escaped = true;
                    } else if bytes[end] == b'"' {
                        return serde_json::from_slice(&bytes[value_start..=end]).ok();
                    }
                    end += 1;
                }
                return None;
            }
            _ => index += 1,
        }
    }
    None
}

fn string_value_after_colon(bytes: &[u8], mut index: usize) -> Option<String> {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index) != Some(&b':') {
        return None;
    }
    index += 1;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index) != Some(&b'"') {
        return None;
    }
    let start = index;
    index += 1;
    let mut escaped = false;
    while index < bytes.len() {
        if escaped {
            escaped = false;
        } else if bytes[index] == b'\\' {
            escaped = true;
        } else if bytes[index] == b'"' {
            return serde_json::from_slice(&bytes[start..=index]).ok();
        }
        index += 1;
    }
    None
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_permission_scanner_ignores_property_text_inside_js_strings_and_comments() {
        for input in [
            r#"text(await tools.exec_command({cmd:"rg 'sandbox_permissions: \\"require_escalated\\"' ."}))"#,
            "// sandbox_permissions: \"require_escalated\"\ntext(true)",
            "/* sandbox_permissions: \"require_escalated\" */ text(true)",
            "text(`sandbox_permissions: \"require_escalated\"`)",
        ] {
            assert_eq!(extract_string_property(input, "sandbox_permissions"), None);
        }
        assert_eq!(
            extract_string_property(
                r#"tools.exec_command({ sandbox_permissions : "require_escalated" })"#,
                "sandbox_permissions",
            )
            .as_deref(),
            Some("require_escalated")
        );
        assert_eq!(
            extract_string_property(
                r#"tools.exec_command({ "sandbox_permissions": "require_escalated" })"#,
                "sandbox_permissions",
            )
            .as_deref(),
            Some("require_escalated")
        );
    }
}
