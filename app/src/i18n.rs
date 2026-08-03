//! Fluent-based localization layer for Zap Desktop.
//!
//! 加载链:
//!   1. `init()` 在启动时调用一次(idempotent),用 `RustEmbed` 加载 `app/i18n/{locale}/*.ftl`
//!   2. `LANGUAGE_LOADER` 是全局 `OnceLock<FluentLanguageLoader>`,在 fallback chain
//!      上 select 当前 locale (默认按系统 locale,可被 settings 覆盖)
//!   3. 业务侧调 `t!("key")` / `t!("key", name = ..)` 取字符串,key 缺失自动回退英文
//!
//! 缺失 key 时:
//!   - 当前 locale 没有 → fluent 内部 fallback 到 fallback_language (en)
//!   - 连英文都没有 → 返回 key 本身字符串(并 log::warn,便于 CI 抓未翻译条目)

#[cfg(not(target_os = "macos"))]
use i18n_embed::DesktopLanguageRequester;
use i18n_embed::{
    fluent::{fluent_language_loader, FluentLanguageLoader},
    LanguageLoader,
};
use rust_embed::RustEmbed;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use unic_langid::LanguageIdentifier;

/// 把 `app/i18n` 目录嵌进二进制。每次构建会重新嵌入(debug-embed feature 已在 workspace 开)。
#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

static LANGUAGE_LOADER: OnceLock<FluentLanguageLoader> = OnceLock::new();

#[derive(Clone, Debug)]
struct SearchTranslation {
    localized: String,
    english: String,
    expand_words: bool,
    context: Option<String>,
}

static SETTINGS_SEARCH_TRANSLATIONS: OnceLock<Mutex<HashMap<String, Vec<SearchTranslation>>>> =
    OnceLock::new();

/// 在 app 启动早期调用一次。
///
/// `override_locale`:用户在 Settings 显式选择的语言(如 "zh-CN"),为 `None` 时按系统 locale。
/// 永远不会 panic — 加载失败会 fallback 到内置英文 bundle。
pub fn init(override_locale: Option<&str>) {
    if LANGUAGE_LOADER.get().is_some() {
        return;
    }

    let loader = fluent_language_loader!();

    // 总是先加载 fallback (en) bundle —— 任何 locale 缺 key 都会落到它。
    if let Err(e) = loader.load_fallback_language(&Localizations) {
        log::error!("[i18n] failed to load fallback (en) bundle: {e}");
    }

    // 决定运行时 locale 列表(按优先级)。
    let requested: Vec<LanguageIdentifier> = match override_locale {
        Some(s) => match s.parse::<LanguageIdentifier>() {
            Ok(li) => vec![li],
            Err(e) => {
                log::warn!("[i18n] invalid override_locale {s:?}: {e} — falling back to system");
                system_requested_languages()
            }
        },
        None => system_requested_languages(),
    };

    if let Err(e) = i18n_embed::select(&loader, &Localizations, &requested) {
        log::warn!("[i18n] select() failed: {e} — running with fallback only");
    }

    log::info!(
        "[i18n] initialized; current_languages={:?}, fallback={}",
        loader.current_languages(),
        loader.fallback_language()
    );

    propagate_ui_locale(&loader);

    let _ = LANGUAGE_LOADER.set(loader);
}

/// Forward the resolved UI locale to `warpui::set_ui_locale` so DirectWrite / CoreText
/// glyph fallback biases CJK Han characters toward the user's UI language. Japanese,
/// Simplified Chinese, and Traditional Chinese share Han code points; without a locale
/// hint, DirectWrite tends to pick Microsoft YaHei (Simplified Chinese) on Windows even
/// when the UI is rendered in Japanese.
fn propagate_ui_locale(loader: &FluentLanguageLoader) {
    let langs = loader.current_languages();
    if let Some(li) = langs.first() {
        warpui::set_ui_locale(li.to_string());
    }
}

fn system_requested_languages() -> Vec<LanguageIdentifier> {
    #[cfg(target_os = "macos")]
    {
        macos_requested_languages()
    }

    #[cfg(not(target_os = "macos"))]
    {
        DesktopLanguageRequester::requested_languages()
    }
}

#[cfg(target_os = "macos")]
fn macos_requested_languages() -> Vec<LanguageIdentifier> {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};
    use warpui::platform::mac::utils::nsstring_as_str;

    unsafe {
        let locale_class = class!(NSLocale);
        let preferred_languages: *const Object = msg_send![locale_class, preferredLanguages];
        let count: usize = msg_send![preferred_languages, count];

        let mut requested = Vec::with_capacity(count);
        for index in 0..count {
            let language: *const Object = msg_send![preferred_languages, objectAtIndex: index];
            match nsstring_as_str(language) {
                Ok(language) => {
                    if let Some(language) = parse_language_identifier(language) {
                        requested.push(language);
                    }
                }
                Err(err) => {
                    log::warn!(
                        "[i18n] failed to read macOS preferred language at index {index}: {err}"
                    );
                }
            }
        }

        languages_or_fallback(requested)
    }
}

fn parse_language_identifier(language: &str) -> Option<LanguageIdentifier> {
    match language.parse::<LanguageIdentifier>() {
        Ok(language) => Some(language),
        Err(err) => {
            log::warn!("[i18n] invalid language identifier {language:?}: {err}");
            None
        }
    }
}

fn languages_or_fallback(languages: Vec<LanguageIdentifier>) -> Vec<LanguageIdentifier> {
    if languages.is_empty() {
        vec![fallback_language()]
    } else {
        languages
    }
}

fn fallback_language() -> LanguageIdentifier {
    "en".parse().expect("en is a valid language identifier")
}

/// 取全局 loader。`init()` 没调过时返回 `None`(早期/测试代码可用 [`t_or`] 兜底)。
pub fn loader() -> Option<&'static FluentLanguageLoader> {
    LANGUAGE_LOADER.get()
}

/// 切换运行时 locale(在 `init()` 之后任意时刻可调)。
///
/// 实现细节:`FluentLanguageLoader::load_languages` 内部用 RwLock 保护语言数据,
/// 故 `&loader` 即可热替换,无需重建。但 **已渲染的 UI 文本不会自动刷新** ——
/// `t!()` 返回的是当时拷贝的 `String`,要看到新语言需要 view 重建/重绘。
/// 调用方可决定是否触发全局重绘,或提示用户重启。
///
/// `locale` 传 BCP-47(如 `"en"`、`"zh-CN"`)。失败时保留原 locale,记录 warn,不 panic。
pub fn set_locale(locale: &str) {
    let Some(loader) = LANGUAGE_LOADER.get() else {
        log::warn!("[i18n] set_locale({locale:?}) called before init() — ignoring");
        return;
    };
    let lang_id: LanguageIdentifier = match locale.parse() {
        Ok(li) => li,
        Err(e) => {
            log::warn!("[i18n] set_locale({locale:?}): invalid BCP-47: {e}");
            return;
        }
    };
    if let Err(e) = loader.load_languages(&Localizations, &[lang_id]) {
        log::warn!("[i18n] set_locale({locale:?}) failed: {e}");
        return;
    }
    log::info!(
        "[i18n] locale switched to {locale:?}; current_languages={:?}",
        loader.current_languages()
    );
    propagate_ui_locale(loader);
}

/// 重置回系统语言(撤销显式 override)。
pub fn reset_to_system_locale() {
    let Some(loader) = LANGUAGE_LOADER.get() else {
        return;
    };
    let requested = system_requested_languages();
    if let Err(e) = i18n_embed::select(loader, &Localizations, &requested) {
        log::warn!("[i18n] reset_to_system_locale failed: {e}");
    }
    propagate_ui_locale(loader);
}

/// 获取已激活的语言列表(主选 + fallback),供设置 UI 与本地化搜索使用。
pub fn current_languages() -> Vec<LanguageIdentifier> {
    LANGUAGE_LOADER
        .get()
        .map(|l| l.current_languages())
        .unwrap_or_default()
}

/// 将当前语言的设置文案反向映射到英文搜索词。
///
/// 设置组件的 `search_terms()` 仍然包含稳定的英文关键词,这里补充当前语言 FTL
/// 文案对应的英文别名与设置分组上下文,让设置搜索不依赖手工维护每种语言的关键词。
pub fn localized_search_aliases(query: &str) -> Vec<Vec<String>> {
    let locale = current_languages()
        .first()
        .map(ToString::to_string)
        .unwrap_or_else(|| "en".to_string());
    search_aliases_for_locale(&locale, query)
}

fn search_aliases_for_locale(locale: &str, query: &str) -> Vec<Vec<String>> {
    let query = query.to_lowercase();
    if query.is_ascii() {
        return query.split_whitespace().map(|_| Vec::new()).collect();
    }

    let translations = search_translations_for_locale(locale);

    query
        .split_whitespace()
        .map(|word| {
            let word = normalize_search_text(word);
            if word.is_empty() || word.is_ascii() {
                return Vec::new();
            }

            let mut aliases = HashSet::new();
            for translation in &translations {
                if !translation.localized.contains(&word) {
                    continue;
                }

                aliases.insert(translation.english.clone());
                if translation.expand_words {
                    if let Some(context) = &translation.context {
                        aliases.insert(context.clone());
                    }
                    aliases.extend(
                        translation
                            .english
                            .split_whitespace()
                            .map(|word| {
                                word.trim_matches(|character: char| !character.is_alphanumeric())
                            })
                            .filter(|word| word.len() >= 2)
                            .map(str::to_owned),
                    );
                }
            }

            let mut aliases = aliases.into_iter().collect::<Vec<_>>();
            aliases.sort_unstable();
            aliases
        })
        .collect()
}

fn search_translations_for_locale(locale: &str) -> Vec<SearchTranslation> {
    let cache = SETTINGS_SEARCH_TRANSLATIONS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(translations) = cache.lock().unwrap().get(locale).cloned() {
        return translations;
    }

    let translations = build_search_translations(locale);
    cache
        .lock()
        .unwrap()
        .insert(locale.to_string(), translations.clone());
    translations
}

fn build_search_translations(locale: &str) -> Vec<SearchTranslation> {
    let english_messages = load_ftl_messages("en");
    let localized_messages = load_ftl_messages(locale);
    let mut translations = Vec::new();

    for (id, localized) in localized_messages {
        if !is_settings_search_message(&id) {
            continue;
        }

        let Some(english) = english_messages.get(&id) else {
            continue;
        };
        let localized = normalize_search_text(&localized);
        let english = normalize_search_text(english);
        if localized.is_empty() || english.is_empty() || localized == english {
            continue;
        }

        translations.push(SearchTranslation {
            localized,
            english,
            expand_words: is_search_label_message(&id),
            context: search_context(&id),
        });
    }

    translations
}

fn load_ftl_messages(locale: &str) -> HashMap<String, String> {
    let mut candidates = vec![format!("{locale}/warp.ftl")];
    if let Some(base_language) = locale.split('-').next() {
        if base_language != locale {
            candidates.push(format!("{base_language}/warp.ftl"));
        }
    }
    if locale.eq_ignore_ascii_case("zh") || locale.starts_with("zh-") {
        candidates.push("zh-CN/warp.ftl".to_string());
    }
    candidates.push("en/warp.ftl".to_string());

    for path in candidates {
        if let Some(file) = Localizations::get(&path) {
            let source = String::from_utf8_lossy(file.data.as_ref());
            return parse_ftl_messages(&source);
        }
    }

    HashMap::new()
}

fn parse_ftl_messages(source: &str) -> HashMap<String, String> {
    let mut messages = HashMap::new();
    let mut current_key = None;
    let mut current_value = String::new();
    let mut in_attribute = false;

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            flush_ftl_message(&mut messages, &mut current_key, &mut current_value);
            in_attribute = false;
            continue;
        }

        if line.starts_with(' ') || line.starts_with('\t') {
            if current_key.is_some() && !in_attribute && !trimmed.starts_with('.') {
                if !current_value.is_empty() {
                    current_value.push(' ');
                }
                current_value.push_str(trimmed);
            } else if trimmed.starts_with('.') {
                in_attribute = true;
            }
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            flush_ftl_message(&mut messages, &mut current_key, &mut current_value);
            in_attribute = false;
            continue;
        };
        let key = key.trim();
        if !is_ftl_message_id(key) {
            flush_ftl_message(&mut messages, &mut current_key, &mut current_value);
            in_attribute = false;
            continue;
        }

        flush_ftl_message(&mut messages, &mut current_key, &mut current_value);
        current_key = Some(key.to_string());
        current_value.push_str(value.trim());
        in_attribute = false;
    }

    flush_ftl_message(&mut messages, &mut current_key, &mut current_value);
    messages
}

fn flush_ftl_message(
    messages: &mut HashMap<String, String>,
    current_key: &mut Option<String>,
    current_value: &mut String,
) {
    if let Some(key) = current_key.take() {
        messages.insert(key, current_value.trim().to_string());
    }
    current_value.clear();
}

fn is_ftl_message_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn normalize_search_text(text: &str) -> String {
    text.replace("\\n", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn is_settings_search_message(id: &str) -> bool {
    id.starts_with("settings-") || id.starts_with("language-widget-")
}

fn search_context(id: &str) -> Option<String> {
    if !is_search_label_message(id) {
        return None;
    }

    let mut segments = id.strip_prefix("settings-")?.split('-');
    let context = segments.next()?;
    segments.next().map(|_| context.to_string())
}

fn is_search_label_message(id: &str) -> bool {
    (id.starts_with("settings-")
        && (id.contains("-category-")
            || id.ends_with("-header")
            || id.ends_with("-label")
            || id.ends_with("-name")
            || id.ends_with("-page-title")
            || id.ends_with("-placeholder")
            || id.ends_with("-subheader")
            || id.ends_with("-subtitle")
            || id.ends_with("-title")))
        || id == "language-widget-label"
}

/// 业务层主入口:`t!("key")` 或 `t!("key", name = value, count = 3)`。
///
/// - 包了 `i18n_embed_fl::fl!`,但额外做了"loader 未初始化"的兜底:
///   返回 key 本身,避免 panic
/// - 返回 `String`(直接喂给 GPUI Text/label_text,无需额外转换)
#[macro_export]
macro_rules! t {
    ($message_id:literal $(,)?) => {{
        match $crate::i18n::loader() {
            Some(loader) => ::i18n_embed_fl::fl!(loader, $message_id),
            None => {
                ::log::warn!(
                    "[i18n] t!({:?}) called before init(); returning key as-is",
                    $message_id
                );
                String::from($message_id)
            }
        }
    }};
    ($message_id:literal, $($args:tt)*) => {{
        match $crate::i18n::loader() {
            Some(loader) => ::i18n_embed_fl::fl!(loader, $message_id, $($args)*),
            None => {
                ::log::warn!(
                    "[i18n] t!({:?}, ...) called before init(); returning key as-is",
                    $message_id
                );
                String::from($message_id)
            }
        }
    }};
}

/// 与 `t!` 等价,但返回 `&'static str`(每次调用都会通过 `Box::leak` 永久占用一段堆)。
///
/// 使用约束:**仅在 `LazyLock`/一次性初始化里调用**(如 `StaticCommand` 这种 struct
/// 字段是 `&'static str`、又必须从 fluent 取文本的场景)。**禁止在热路径或循环里使用**,
/// 否则会持续泄漏内存。编译期仍享受 `fl!()` 的 key 校验。
#[macro_export]
macro_rules! t_static {
    ($message_id:literal $(,)?) => {{
        let s: String = $crate::t!($message_id);
        &*::std::boxed::Box::leak(s.into_boxed_str())
    }};
}

/// 同 `t!` 但带显式默认值,适合极早期/loader 未就绪场景。
pub fn t_or(message_id: &str, fallback: &str) -> String {
    match LANGUAGE_LOADER.get() {
        Some(loader) if loader.has(message_id) => loader.get(message_id),
        _ => fallback.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent() {
        init(Some("en"));
        init(Some("en"));
        assert!(loader().is_some());
    }

    #[test]
    fn fallback_chain_works() {
        init(Some("zh-CN"));
        let loader = loader().unwrap();
        // common-ok 中文已译
        assert_eq!(loader.get("common-ok"), "确定");
        // 不存在的 key — fluent 会返回 key 本身或带 marker 的字符串
        let missing = loader.get("definitely-does-not-exist");
        assert!(missing.contains("definitely-does-not-exist"));
    }

    #[test]
    fn requested_languages_keep_preferred_order() {
        let languages = ["ja", "zh-CN"]
            .into_iter()
            .filter_map(parse_language_identifier)
            .collect();

        let languages = languages_or_fallback(languages);

        assert_eq!(languages[0].to_string(), "ja");
        assert_eq!(languages[1].to_string(), "zh-CN");
    }

    #[test]
    fn requested_languages_fall_back_to_english_when_empty() {
        let languages = languages_or_fallback(Vec::new());

        assert_eq!(languages.len(), 1);
        assert_eq!(languages[0].to_string(), "en");
    }

    #[test]
    fn parses_ftl_messages_without_attributes() {
        let messages = parse_ftl_messages(
            "settings-test-label = 中文搜索\n    第二行\n    .accesskey = X\n\nother = ignored",
        );

        assert_eq!(
            messages.get("settings-test-label"),
            Some(&"中文搜索 第二行".to_string())
        );
    }

    #[test]
    fn zh_cn_search_aliases_map_to_english_terms() {
        let aliases = search_aliases_for_locale("zh-CN", "代理模式");

        assert!(aliases.iter().any(|alias| alias == "proxy"));

        let aliases = search_aliases_for_locale("zh-CN", "用户名");
        assert!(aliases.iter().any(|alias| alias == "network"));
    }
}
