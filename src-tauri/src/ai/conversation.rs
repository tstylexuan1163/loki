//! Limits applied when a conversation is saved.
//!
//! A conversation holds tool output, which can be megabytes (a `cat` of a large
//! file). Storing that would bloat the vault and slow every sync, so results are
//! capped on the way in. The last turns are kept when a conversation grows too
//! long: continuing a chat needs its recent context, not the beginning.

use serde_json::Value;

/// Longest tool result kept, per call.
pub const MAX_TOOL_RESULT: usize = 8 * 1024;
/// Most turns kept in one conversation.
pub const MAX_TURNS: usize = 200;
/// Most conversations kept on this machine.
pub const MAX_CONVERSATIONS: usize = 200;

const MARKER: &str = "\n…已截断";

/// Cuts `text` to at most `limit` bytes on a UTF-8 boundary and marks the cut.
fn cut(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{MARKER}", &text[..end])
}

/// Applies the size limits in place and returns the payload.
///
/// Expects the same camelCase shape the panel sends: `turns[].parts[]`, with a
/// tool part shaped `{ call: { result: { content } } }`.
pub fn truncate(mut payload: Value) -> Value {
    let Some(turns) = payload.get_mut("turns").and_then(|t| t.as_array_mut()) else {
        return payload;
    };

    if turns.len() > MAX_TURNS {
        let drop = turns.len() - MAX_TURNS;
        turns.drain(0..drop);
    }

    for turn in turns.iter_mut() {
        let Some(parts) = turn.get_mut("parts").and_then(|p| p.as_array_mut()) else {
            continue;
        };
        for part in parts.iter_mut() {
            if let Some(slot) = part.pointer_mut("/call/result/content") {
                if let Some(text) = slot.as_str() {
                    if text.len() > MAX_TOOL_RESULT {
                        *slot = Value::String(cut(text, MAX_TOOL_RESULT));
                    }
                }
            }
        }
    }

    payload
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_part(content: &str) -> Value {
        json!({
            "kind": "tool",
            "call": {
                "id": "c1",
                "name": "run_command",
                "args": { "command": "cat big" },
                "auto": true,
                "result": { "ok": true, "content": content }
            }
        })
    }

    #[test]
    fn long_tool_result_is_truncated_with_a_marker() {
        let big = "x".repeat(MAX_TOOL_RESULT * 3);
        let payload = json!({
            "id": "c1",
            "turns": [{ "role": "assistant", "parts": [tool_part(&big)] }]
        });
        let out = truncate(payload);
        let content = out["turns"][0]["parts"][0]["call"]["result"]["content"]
            .as_str()
            .unwrap();
        assert!(content.len() <= MAX_TOOL_RESULT + MARKER.len() + 8);
        assert!(content.ends_with(MARKER.trim_start_matches('\n')));
    }

    #[test]
    fn short_results_and_text_are_left_alone() {
        let payload = json!({
            "id": "c1",
            "turns": [{
                "role": "assistant",
                "parts": [
                    { "kind": "text", "text": "hello" },
                    tool_part("small output")
                ]
            }]
        });
        let out = truncate(payload.clone());
        assert_eq!(out, payload);
    }

    #[test]
    fn truncation_respects_utf8_boundaries() {
        // 4 bytes each; cutting at a byte offset must not split a character.
        let text = "好".repeat(MAX_TOOL_RESULT);
        let payload = json!({ "turns": [{ "parts": [tool_part(&text)] }] });
        let out = truncate(payload);
        let content = out["turns"][0]["parts"][0]["call"]["result"]["content"]
            .as_str()
            .unwrap();
        assert!(content.starts_with('好'));
        // Still valid UTF-8 (it parsed out of the Value, which proves it).
        assert!(content.len() <= MAX_TOOL_RESULT + MARKER.len() + 8);
    }

    #[test]
    fn only_the_most_recent_turns_are_kept() {
        let turns: Vec<Value> = (0..MAX_TURNS + 25)
            .map(
                |i| json!({ "role": "user", "parts": [{ "kind": "text", "text": i.to_string() }] }),
            )
            .collect();
        let out = truncate(json!({ "turns": turns }));
        let kept = out["turns"].as_array().unwrap();
        assert_eq!(kept.len(), MAX_TURNS);
        // The first kept turn is number 25, i.e. the oldest were dropped.
        assert_eq!(kept[0]["parts"][0]["text"], "25");
    }

    #[test]
    fn payload_without_turns_is_untouched() {
        let payload = json!({ "id": "c1", "title": "t" });
        assert_eq!(truncate(payload.clone()), payload);
    }
}
