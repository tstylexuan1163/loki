//! Keeping a conversation within the model's context window.
//!
//! Every turn sends the whole conversation again — chat APIs are stateless —
//! and providers cache a repeated prefix, so resending is cheap as long as the
//! start of the conversation does not change. Nothing is cut, therefore, until
//! the conversation actually nears the model's limit. Then, cheapest first and
//! without an extra model call:
//! 1. Tool output is cut to its head and tail, oldest rounds first, the last
//!    few rounds last: older output is what the model needs least.
//! 2. Whole rounds are dropped from the start, never the latest, and the first
//!    kept question says so.
//!
//! The limit comes from the provider when it says (its model list, or the
//! error it returns for an over-long request; see `limit_from_error`), or from
//! the user. Unknown means no trimming: the first overflow teaches it.
//!
//! A round is a user message and everything up to the next one, so a tool
//! result never loses the assistant message that asked for it.

use super::provider::{ChatMessage, Role};

/// Rounds at the end whose tool output is cut last.
pub const RECENT_ROUNDS: usize = 2;
/// Longest tool output kept from the recent rounds once trimming starts.
pub const RECENT_CAP: usize = 12_000;
/// Longest tool output kept from older rounds once trimming starts.
pub const OLD_CAP: usize = 1_200;
/// What the system prompt and the tool schemas take, roughly.
const OVERHEAD: usize = 3_000;

/// What was left out, for the panel to show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trimmed {
    pub dropped_rounds: usize,
    pub shortened_outputs: usize,
    /// Estimated size of what is sent.
    pub tokens: usize,
}

/// How much history fits in a model with `limit` tokens of context: room is
/// kept for the reply, the system prompt and the tool schemas, and a margin
/// for the estimate being an estimate.
pub fn budget_for(limit: usize) -> usize {
    let reply = (limit / 4).min(8_192);
    limit.saturating_sub(reply + OVERHEAD) * 9 / 10
}

/// A token estimate good enough for budgeting: a CJK character is about one
/// token, other text about one per 3.5 characters.
pub fn estimate(text: &str) -> usize {
    let mut cjk = 0usize;
    let mut other = 0usize;
    for ch in text.chars() {
        if ('\u{2E80}'..='\u{9FFF}').contains(&ch)
            || ('\u{F900}'..='\u{FAFF}').contains(&ch)
            || ('\u{FF00}'..='\u{FFEF}').contains(&ch)
        {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    cjk + other.div_ceil(4) + other / 28
}

fn message_tokens(m: &ChatMessage) -> usize {
    let calls: usize = m
        .tool_calls
        .iter()
        .map(|c| estimate(&c.name) + estimate(&c.arguments.to_string()))
        .sum();
    estimate(&m.content) + calls + 4
}

/// Total estimate for a message list.
pub fn total(messages: &[ChatMessage]) -> usize {
    messages.iter().map(message_tokens).sum()
}

/// Keeps the start and end of `text`, at most `cap` characters in all.
fn shorten(text: &str, cap: usize, note: &str) -> Option<String> {
    let n = text.chars().count();
    if n <= cap {
        return None;
    }
    let head = cap * 2 / 3;
    let tail = cap - head;
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(n - tail).collect();
    Some(format!(
        "{start}\n…（{note}，省略了 {} 个字符）…\n{end}",
        n - cap
    ))
}

/// Index where each round starts (each user message).
fn round_starts(messages: &[ChatMessage]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == Role::User)
        .map(|(i, _)| i)
        .collect()
}

const NOTE: &str = "为控制长度，这段输出已精简，需要完整内容可以重新执行";

/// Cuts tool outputs in `indices` (in order) to `cap` until within `budget`.
fn shorten_until(
    history: &mut [ChatMessage],
    indices: &[usize],
    cap: usize,
    budget: usize,
    report: &mut Trimmed,
) {
    for &i in indices {
        if total(history) <= budget {
            return;
        }
        if let Some(short) = shorten(&history[i].content, cap, NOTE) {
            history[i].content = short;
            report.shortened_outputs += 1;
        }
    }
}

/// Fits `history` (no system prompt) within `budget` estimated tokens. A
/// history that already fits comes back untouched.
pub fn fit(mut history: Vec<ChatMessage>, budget: usize) -> (Vec<ChatMessage>, Trimmed) {
    let mut report = Trimmed::default();
    if total(&history) > budget {
        let starts = round_starts(&history);
        let recent_from = starts
            .len()
            .checked_sub(RECENT_ROUNDS)
            .and_then(|i| starts.get(i).copied())
            .unwrap_or(0);
        let tools: Vec<usize> = (0..history.len())
            .filter(|&i| history[i].role == Role::Tool)
            .collect();
        let (old, recent): (Vec<usize>, Vec<usize>) = tools.iter().partition(|&&i| i < recent_from);

        // 1. Old output first, then recent output, oldest first in each.
        shorten_until(&mut history, &old, OLD_CAP, budget, &mut report);
        shorten_until(&mut history, &recent, RECENT_CAP, budget, &mut report);

        // 2. Whole rounds from the start, keeping the last.
        while total(&history) > budget {
            let starts = round_starts(&history);
            if starts.len() < 2 {
                break;
            }
            // Everything before the second round goes: the first round, plus
            // any stray messages that came before it.
            history.drain(..starts[1]);
            report.dropped_rounds += 1;
        }

        // 3. One round that still does not fit: cut its output harder.
        if total(&history) > budget {
            let rest: Vec<usize> = (0..history.len())
                .filter(|&i| history[i].role == Role::Tool)
                .collect();
            shorten_until(&mut history, &rest, OLD_CAP, budget, &mut report);
        }

        if report.dropped_rounds > 0 {
            if let Some(first) = history.iter_mut().find(|m| m.role == Role::User) {
                first.content = format!(
                    "（为控制长度，这段对话更早的 {} 轮没有发送。需要更早的信息时请向用户询问，或重新执行命令。）\n\n{}",
                    report.dropped_rounds, first.content
                );
            }
        }
    }
    report.tokens = total(&history);
    (history, report)
}

/// Whether a provider's error says the request was longer than the model
/// takes. Providers word it differently; these are the phrasings in use by
/// OpenAI-compatible servers (OpenAI, DeepSeek, vLLM, OpenRouter, LM Studio)
/// and Anthropic.
pub fn is_overflow(error: &str) -> bool {
    let e = error.to_lowercase();
    [
        "context length",
        "context_length",
        "context window",
        "maximum context",
        "prompt is too long",
        "input is too long",
        "too many tokens",
        "reduce the length",
        "exceeds the model",
        "exceed the context",
        "tokens exceed",
        "上下文长度",
    ]
    .iter()
    .any(|k| e.contains(k))
}

/// Numbers in `text` with the byte offset where each starts.
fn numbers(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut value: usize = 0;
    for (i, ch) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        match ch.to_digit(10) {
            Some(d) => {
                start.get_or_insert(i);
                value = value.saturating_mul(10).saturating_add(d as usize);
            }
            // A thousands separator inside a number ("128,000").
            None if ch == ','
                && start.is_some()
                && text[i + 1..].starts_with(|c: char| c.is_ascii_digit()) => {}
            None => {
                if let Some(s) = start.take() {
                    out.push((s, value));
                }
                value = 0;
            }
        }
    }
    out
}

/// The model's context size, when the provider's overflow error states it.
///
/// - OpenAI-style: "This model's maximum context length is 65536 tokens. However,
///   you requested 70000 tokens…" — the number after "maximum context length".
/// - Anthropic: "prompt is too long: 210000 tokens > 200000 maximum" — the
///   number before "maximum".
/// - Others: "context window of 32768 tokens", "limit of 8192 tokens".
pub fn limit_from_error(error: &str) -> Option<usize> {
    let lower = error.to_lowercase();
    let nums = numbers(&lower);
    let after = |phrase: &str| {
        let at = lower.find(phrase)? + phrase.len();
        nums.iter().find(|(pos, _)| *pos >= at).map(|(_, v)| *v)
    };
    let before = |phrase: &str| {
        let at = lower.find(phrase)?;
        nums.iter()
            .rev()
            .find(|(pos, _)| *pos < at)
            .map(|(_, v)| *v)
    };
    let found = after("maximum context length")
        .or_else(|| after("context length of"))
        .or_else(|| after("context window of"))
        .or_else(|| after("context length is"))
        .or_else(|| after("limit of"))
        .or_else(|| after("maximum of"))
        .or_else(|| before("maximum"));
    // Anything this small is not a context window; it is a stray number.
    found.filter(|&n| n >= 1_024)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::provider::ToolCall;

    fn user(t: &str) -> ChatMessage {
        ChatMessage::user(t)
    }

    fn asks(id: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: id.into(),
                name: "run_command".into(),
                arguments: serde_json::json!({ "command": "df -h" }),
            }],
            tool_call_id: None,
        }
    }

    fn result(id: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Tool,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: Some(id.into()),
        }
    }

    fn answer(t: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: t.into(),
            tool_calls: vec![],
            tool_call_id: None,
        }
    }

    /// `n` rounds, each with one command whose output is `size` characters.
    fn conversation(n: usize, size: usize) -> Vec<ChatMessage> {
        let mut out = Vec::new();
        for i in 0..n {
            out.push(user(&format!("问题 {i}")));
            out.push(asks(&format!("c{i}")));
            out.push(result(&format!("c{i}"), &"x".repeat(size)));
            out.push(answer("好的"));
        }
        out
    }

    #[test]
    fn estimate_counts_cjk_as_about_one_token_each() {
        assert_eq!(estimate("你好世界"), 4);
        let ascii = estimate(&"a".repeat(350));
        assert!((90..=110).contains(&ascii), "{ascii}");
    }

    #[test]
    fn budget_leaves_room_for_the_reply() {
        assert!(budget_for(1_000_000) > 880_000);
        assert!(budget_for(1_000_000) < 1_000_000);
        let small = budget_for(8_192);
        assert!((2_000..6_000).contains(&small), "{small}");
        assert_eq!(budget_for(1_000), 0);
    }

    #[test]
    fn a_conversation_within_budget_is_sent_untouched() {
        // Big outputs, but a big model: nothing changes, so the prefix the
        // provider cached stays valid.
        let history = conversation(20, 20_000);
        let (out, report) = fit(history.clone(), budget_for(1_000_000));
        assert_eq!(
            report,
            Trimmed {
                tokens: total(&history),
                ..Default::default()
            }
        );
        for (a, b) in out.iter().zip(&history) {
            assert_eq!(a.content, b.content);
        }
    }

    #[test]
    fn oldest_output_goes_first_and_only_as_much_as_needed() {
        let history = conversation(6, 8_000);
        let before = total(&history);
        // A budget a little under the total: shortening one or two of the
        // oldest outputs is enough.
        let (out, report) = fit(history, before - 3_000);
        assert!(
            out[2].content.contains("已精简"),
            "the oldest output was not cut"
        );
        assert_eq!(
            out[22].content,
            "x".repeat(8_000),
            "the newest output was cut"
        );
        assert!((1..=2).contains(&report.shortened_outputs), "{report:?}");
        assert_eq!(report.dropped_rounds, 0);
        assert!(report.tokens <= before - 3_000);
    }

    #[test]
    fn rounds_are_dropped_from_the_start_and_the_model_is_told() {
        let mut history = Vec::new();
        for i in 0..60 {
            history.push(user(&format!("问题 {i} {}", "很长的问题".repeat(400))));
            history.push(answer("好的"));
        }
        let (out, report) = fit(history, 40_000);
        assert!(report.dropped_rounds > 0);
        assert!(report.tokens <= 40_000, "{}", report.tokens);
        assert!(
            out[0].content.starts_with("（为控制长度"),
            "{}",
            &out[0].content[..40]
        );
        assert!(out.iter().any(|m| m.content.contains("问题 59")));
        assert_eq!(out[0].role, Role::User);
    }

    #[test]
    fn tool_results_never_lose_their_call() {
        let (out, _) = fit(conversation(40, 20_000), 6_000);
        for (i, m) in out.iter().enumerate() {
            if m.role == Role::Tool {
                let id = m.tool_call_id.as_deref().unwrap();
                assert!(
                    out[..i]
                        .iter()
                        .any(|p| p.tool_calls.iter().any(|c| c.id == id)),
                    "result {id} without its call"
                );
            }
        }
    }

    #[test]
    fn a_single_round_is_never_dropped_but_its_output_is_cut() {
        // Cut to the recent cap is enough for this budget…
        let (out, report) = fit(conversation(1, 200_000), 5_000);
        assert_eq!(out.len(), 4);
        assert_eq!(report.dropped_rounds, 0);
        assert!(out[2].content.chars().count() < RECENT_CAP + 200);
        assert!(report.tokens <= 5_000, "{}", report.tokens);
        // …a tighter one cuts it to the old cap.
        let (out, report) = fit(conversation(1, 200_000), 1_000);
        assert_eq!(out.len(), 4);
        assert!(out[2].content.chars().count() < OLD_CAP + 200);
        assert!(report.tokens <= 1_000, "{}", report.tokens);
    }

    #[test]
    fn overflow_errors_are_recognised() {
        for e in [
            "This model's maximum context length is 65536 tokens. However, you requested 70123 tokens",
            "prompt is too long: 210000 tokens > 200000 maximum",
            "{\"error\":{\"code\":\"context_length_exceeded\"}}",
            "Please reduce the length of the messages or completion.",
            "Trying to keep the first 9000 tokens when context the overflows. However, the model is loaded with context length of 8192",
        ] {
            assert!(is_overflow(e), "{e}");
        }
        assert!(!is_overflow("Incorrect API key provided"));
        assert!(!is_overflow("rate limit exceeded, retry in 20s"));
    }

    #[test]
    fn the_limit_is_read_from_the_error() {
        assert_eq!(
            limit_from_error("This model's maximum context length is 65536 tokens. However, you requested 70123 tokens"),
            Some(65_536)
        );
        assert_eq!(
            limit_from_error("prompt is too long: 210000 tokens > 200000 maximum"),
            Some(200_000)
        );
        assert_eq!(
            limit_from_error("the model is loaded with context length of 8192"),
            Some(8_192)
        );
        assert_eq!(
            limit_from_error("exceeds the context window of 1,048,576 tokens"),
            Some(1_048_576)
        );
        assert_eq!(limit_from_error("context_length_exceeded"), None);
        // HTTP status codes are not context windows.
        assert_eq!(
            limit_from_error("400 Bad Request: maximum context exceeded"),
            None
        );
    }
}
