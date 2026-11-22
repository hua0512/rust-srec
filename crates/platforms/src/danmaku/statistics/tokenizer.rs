//! Tokenization, case folding, and stop-word filtering for danmaku text.

use std::borrow::Cow;
use std::sync::LazyLock;

use jieba_rs::Jieba;
use rustc_hash::FxHashSet;

/// Livestream vocabulary absent from jieba's general-Chinese dictionary that its
/// HMM also does not infer, so without seeding it the words split into single
/// characters and `StatisticsAggregator::record_word` discards them as noise.
///
/// A seed, not an exhaustive list: anything missing still segments per-character
/// and simply goes uncounted rather than producing a wrong word.
const DANMU_DICT_WORDS: &[&str] = &[
    "牛逼",
    "弹幕",
    "下播",
    "开播",
    "直播间",
    "老铁",
    "破防",
    "泪目",
    "上分",
    "团战",
    "打野",
    "中单",
    "野区",
    "残血",
    "反杀",
    "秒了",
    "抬走",
    "整活",
];

/// Shared segmenter for CJK word frequency.
///
/// `Jieba` is immutable after construction and `cut` takes `&self`, so one
/// instance serves every concurrent session. `LazyLock` keeps the embedded
/// dictionary out of memory entirely for deployments whose danmu is never CJK.
static JIEBA: LazyLock<Jieba> = LazyLock::new(|| {
    let mut jieba = Jieba::new();
    for word in DANMU_DICT_WORDS {
        jieba.add_word(word, None, None);
    }
    jieba
});

pub(super) static STOP_WORDS: LazyLock<FxHashSet<&'static str>> = LazyLock::new(default_stop_words);

/// Whether `text` contains a character that needs dictionary segmentation.
///
/// Covers CJK Unified Ideographs (plus the common extension A), Japanese
/// hiragana/katakana, and the CJK compatibility block. Hangul is excluded:
/// Korean is written with spaces, so the outer split already tokenizes it.
pub(super) fn contains_cjk(text: &str) -> bool {
    if text.is_ascii() {
        return false;
    }
    text.chars().any(|c| {
        matches!(c,
            '\u{3040}'..='\u{30FF}'      // hiragana + katakana
            | '\u{3400}'..='\u{4DBF}'    // CJK ext. A
            | '\u{4E00}'..='\u{9FFF}'    // CJK unified ideographs
            | '\u{F900}'..='\u{FAFF}'    // CJK compatibility ideographs
        )
    })
}

/// Visits candidate tokens in `content`, segmenting CJK runs with `JIEBA` and
/// keeping non-CJK alphanumeric runs allocation-free.
pub(super) fn for_each_token<'a>(content: &'a str, mut visit: impl FnMut(&'a str)) {
    for run in content
        .split(|c: char| !c.is_alphanumeric())
        // Single-character words are discarded, so dropping single-char runs here
        // keeps a lone CJK character from loading the dictionary.
        .filter(|run| run.chars().nth(1).is_some())
    {
        if contains_cjk(run) {
            // HMM inference on: the bundled dictionary is general Chinese, and
            // without it a common word like `主播` splits into `主` + `播`,
            // which `normalize_word` then discards as single-character noise.
            // `Token::word` borrows from `run`, so segments cost no allocation
            // beyond the returned Vec.
            for token in JIEBA.cut(run, true) {
                visit(token.word);
            }
        } else {
            visit(run);
        }
    }
}

/// Validates minimum length (>= 2 characters) and case-folds `word` without
/// allocating when it is already lowercase or caseless (such as CJK).
pub(super) fn normalize_word(word: &str) -> Option<Cow<'_, str>> {
    let mut chars = word.chars();
    chars.next()?;
    chars.next()?;

    if word.is_ascii() {
        if word.bytes().any(|b| b.is_ascii_uppercase()) {
            Some(Cow::Owned(word.to_ascii_lowercase()))
        } else {
            Some(Cow::Borrowed(word))
        }
    } else if word.chars().any(char::is_uppercase) {
        Some(Cow::Owned(word.to_lowercase()))
    } else {
        Some(Cow::Borrowed(word))
    }
}

/// Default stop words for filtering.
fn default_stop_words() -> FxHashSet<&'static str> {
    let words = [
        // English
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "must", "shall",
        "can", "need", "dare", "to", "of", "in", "for", "on", "with", "at", "by", "from", "as",
        "into", "through", "during", "before", "after", "above", "below", "between", "under",
        "again", "further", "then", "once", "here", "there", "when", "where", "why", "how", "all",
        "each", "few", "more", "most", "other", "some", "such", "no", "nor", "not", "only", "own",
        "same", "so", "than", "too", "very", "just", "and", "but", "if", "or", "because", "until",
        "while", "this", "that", "these", "those", "it", "its", "he", "she", "they", "them", "his",
        "her", "their", "what", "which", "who", "whom", // Chinese common words
        "的", "了", "是", "在", "我", "有", "和", "就", "不", "人", "都", "一", "一个", "上", "也",
        "很", "到", "说", "要", "去", "你", "会", "着", "没有", "看", "好", "自己", "这", "那",
        // Common chat expressions
        "lol", "lmao", "haha", "hehe", "xd", "gg", "ez", "wp", "666", "233", "哈哈", "呵呵", "嘿嘿",
    ];

    words.into_iter().collect()
}
