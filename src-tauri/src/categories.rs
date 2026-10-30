//! Host categories ("服务器", "交换机", anything the user names).
//!
//! A host lists its categories by name in its `tags` field — the same field
//! older versions used for free-form tags, so every existing tag simply becomes
//! a category. The category records themselves (`host_category` items) carry
//! what a name cannot: the display order and an optional colour.
//!
//! Everything here is pure; the vault reads and writes live in `commands.rs`.

/// Item kind of a category record.
pub const KIND: &str = "host_category";

/// Longest category name accepted.
pub const MAX_NAME: usize = 32;

/// Same name, ignoring case and surrounding blanks.
pub fn same(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Trims names, drops blanks and case-insensitive duplicates (first one wins).
pub fn normalize(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let t = t.trim();
        if !t.is_empty() && !out.iter().any(|o| same(o, t)) {
            out.push(t.to_string());
        }
    }
    out
}

/// A category name as typed, or why it cannot be one.
pub fn valid_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("分类名称不能为空".into());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("分类名称最多 {MAX_NAME} 个字"));
    }
    Ok(name.to_string())
}

/// `tags` with `old` replaced by `new`, keeping its position.
pub fn rename(tags: &[String], old: &str, new: &str) -> Vec<String> {
    let swapped: Vec<String> = tags
        .iter()
        .map(|t| {
            if same(t, old) {
                new.to_string()
            } else {
                t.clone()
            }
        })
        .collect();
    // Renaming onto a name the host already had merges the two.
    normalize(&swapped)
}

/// `tags` without `name`.
pub fn remove(tags: &[String], name: &str) -> Vec<String> {
    tags.iter().filter(|t| !same(t, name)).cloned().collect()
}

/// Names used by hosts that have no category record yet, in first-seen order.
pub fn missing<'a>(known: &[String], used: impl IntoIterator<Item = &'a String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in used {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if !known.iter().any(|k| same(k, name)) && !out.iter().any(|o| same(o, name)) {
            out.push(name.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn normalize_trims_and_dedupes_ignoring_case() {
        assert_eq!(
            normalize(&v(&[" 交换机 ", "", "Prod", "prod", "交换机"])),
            v(&["交换机", "Prod"])
        );
    }

    #[test]
    fn names_are_checked() {
        assert_eq!(valid_name("  服务器 ").unwrap(), "服务器");
        assert!(valid_name("   ").is_err());
        assert!(valid_name(&"长".repeat(MAX_NAME + 1)).is_err());
        assert!(valid_name(&"长".repeat(MAX_NAME)).is_ok());
    }

    #[test]
    fn rename_keeps_position_and_merges_duplicates() {
        assert_eq!(
            rename(&v(&["a", "switch", "b"]), "Switch", "交换机"),
            v(&["a", "交换机", "b"])
        );
        // The host already had the target name: no duplicate left behind.
        assert_eq!(
            rename(&v(&["交换机", "switch"]), "switch", "交换机"),
            v(&["交换机"])
        );
        assert_eq!(rename(&v(&["a"]), "x", "y"), v(&["a"]));
    }

    #[test]
    fn remove_drops_only_that_name() {
        assert_eq!(
            remove(&v(&["服务器", "Lab", "lab2"]), "lab"),
            v(&["服务器", "lab2"])
        );
    }

    #[test]
    fn missing_lists_unknown_names_once_in_order() {
        let used = v(&["prod", "交换机", "Prod", " ", "lab"]);
        assert_eq!(missing(&v(&["LAB"]), &used), v(&["prod", "交换机"]));
    }
}
