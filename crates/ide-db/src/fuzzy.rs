//! 模糊匹配打分：子序列匹配 + 驼峰 / 词首 / 连续命中加分。
//!
//! 用于符号搜索和补全排序。分数越高越好；不匹配返回 `None`。

/// `query` 是否按顺序出现在 `candidate` 里；匹配时给出分数。
/// 大小写规则：查询里的小写字母不区分大小写，大写字母必须匹配大写（"智能大小写"）。
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let query: Vec<char> = query.chars().collect();
    let chars: Vec<char> = candidate.chars().collect();

    let mut score = 0i32;
    let mut qi = 0usize;
    let mut prev_matched = false;
    let mut first_match: Option<usize> = None;

    for (ci, &c) in chars.iter().enumerate() {
        if qi == query.len() {
            break;
        }
        let q = query[qi];
        let matches = if q.is_uppercase() {
            c == q
        } else {
            c.to_lowercase().eq(q.to_lowercase())
        };
        if !matches {
            prev_matched = false;
            continue;
        }
        first_match.get_or_insert(ci);

        let at_word_start = ci == 0
            || matches!(chars[ci - 1], '_' | '$' | '.' | '/')
            || (c.is_uppercase() && !chars[ci - 1].is_uppercase())
            || (c.is_ascii_digit() != chars[ci - 1].is_ascii_digit());
        score += 10;
        if at_word_start {
            score += 20;
        }
        if prev_matched {
            score += 15;
        }
        if c == q {
            score += 2;
        }
        prev_matched = true;
        qi += 1;
    }
    if qi < query.len() {
        return None;
    }

    // 从头匹配、完全匹配、短候选优先
    if first_match == Some(0) {
        score += 40;
    }
    if chars.len() == query.len() {
        score += 60;
    }
    score -= (chars.len() as i32 - query.len() as i32).min(40);
    Some(score)
}

/// 补全用的前缀匹配：`prefix` 为空时全部匹配。
pub fn matches_prefix(prefix: &str, candidate: &str) -> bool {
    fuzzy_score(prefix, candidate).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_and_ranking() {
        assert!(fuzzy_score("asl", "ArrayList").is_none());
        let exact = fuzzy_score("List", "List").unwrap();
        let prefix = fuzzy_score("List", "ListIterator").unwrap();
        let inner = fuzzy_score("List", "ArrayList").unwrap();
        assert!(exact > prefix && prefix > inner);
    }

    #[test]
    fn camel_humps() {
        let humps = fuzzy_score("NPE", "NullPointerException").unwrap();
        assert!(humps > 0);
        assert!(
            fuzzy_score("NPE", "nullpointerexception").is_none(),
            "大写字母必须匹配大写"
        );
        assert!(fuzzy_score("npe", "NullPointerException").is_some());
        let scattered = fuzzy_score("npe", "inputProcessEvent").unwrap();
        assert!(fuzzy_score("npe", "NullPointerException").unwrap() > scattered);
    }
}
