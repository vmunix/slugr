//! Slug generator for filenames and arbitrary text.
//!
//! Two entry points:
//!
//! - [`slugify()`] — filename-aware: preserves extensions, dotfiles, compound
//!   extensions (`.tar.gz`), and version numbers (`1.2.3`).
//! - [`slugify_string()`] — plain text: no extension splitting or dotfile
//!   handling. Use for URL slugs, identifiers, titles, etc.
//!
//! # Examples
//!
//! ```
//! use fileslug::{slugify, slugify_string, SlugifyOptions};
//!
//! let opts = SlugifyOptions::default();
//! assert_eq!(slugify("My Résumé (Final).pdf", &opts), "my-resume-final.pdf");
//! assert_eq!(slugify(".gitignore", &opts), ".gitignore");
//! assert_eq!(slugify("app-1.2.3.dmg", &opts), "app-1.2.3.dmg");
//!
//! assert_eq!(slugify_string("My Blog Post Title!", &opts), "my-blog-post-title");
//! assert_eq!(slugify_string("Café Résumé", &opts), "cafe-resume");
//! ```

use std::borrow::Cow;

/// Split a filename into `(base, extension)`.
///
/// Handles compound extensions (`.tar.gz`, `.tar.bz2`, `.tar.xz`, `.tar.zst`),
/// dotfiles, and files with no extension. Dotfiles like `.gitignore` are treated
/// as having no base — the entire name is the "extension".
///
/// # Examples
///
/// ```
/// use fileslug::split_extension;
///
/// assert_eq!(split_extension("report.pdf"), ("report", ".pdf"));
/// assert_eq!(split_extension("archive.tar.gz"), ("archive", ".tar.gz"));
/// assert_eq!(split_extension(".gitignore"), ("", ".gitignore"));
/// assert_eq!(split_extension("Makefile"), ("Makefile", ""));
/// ```
#[must_use]
pub fn split_extension(filename: &str) -> (&str, &str) {
    const COMPOUND: &[&str] = &[".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst"];

    // Dotfiles with no further extension: .gitignore, .env, .bashrc
    if filename.starts_with('.') && !filename[1..].contains('.') {
        return ("", filename);
    }

    // Compound extensions
    let lower = filename.to_lowercase();
    for ext in COMPOUND {
        if lower.ends_with(ext) {
            let base_end = filename.len() - ext.len();
            return (&filename[..base_end], &filename[base_end..]);
        }
    }

    // Simple extension: split at last dot
    match filename.rfind('.') {
        Some(pos) if pos > 0 => (&filename[..pos], &filename[pos..]),
        _ => (filename, ""),
    }
}

/// Word separator style for slugified filenames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Style {
    /// `my-cool-file.txt` (default)
    #[default]
    Kebab,
    /// `my_cool_file.txt`
    Snake,
    /// `MyCoolFile.txt`
    Pascal,
}

/// Options controlling the [`slugify`] pipeline.
///
/// # Examples
///
/// ```
/// use fileslug::{slugify, SlugifyOptions, Style};
///
/// // Snake case, with unicode transliteration (default)
/// let opts = SlugifyOptions { style: Style::Snake, ..Default::default() };
/// assert_eq!(slugify("My Résumé.pdf", &opts), "my_resume.pdf");
///
/// // Kebab case, keeping unicode intact
/// let opts = SlugifyOptions { keep_unicode: true, ..Default::default() };
/// assert_eq!(slugify("Café Menu.txt", &opts), "café-menu.txt");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlugifyOptions {
    /// Word separator style (kebab, snake, or pascal).
    pub style: Style,
    /// When `true`, skip ASCII transliteration and preserve unicode characters.
    pub keep_unicode: bool,
}

impl Default for SlugifyOptions {
    fn default() -> Self {
        Self {
            style: Style::Kebab,
            keep_unicode: false,
        }
    }
}

/// Placeholder byte used to protect dots inside version numbers.
const VERSION_DOT: char = '\x01';

/// Replace dots inside version-like sequences (e.g. "0.8.34") with a placeholder
/// so the word-splitting step doesn't break them apart.
///
/// A version sequence is `\d+(\.\d+)+` — two or more digit groups separated by dots.
fn preserve_version_dots(input: &str) -> String {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut result = String::with_capacity(len);
    let mut i = 0;

    while i < len {
        if bytes[i].is_ascii_digit() {
            let start = i;

            // Consume first digit group
            while i < len && bytes[i].is_ascii_digit() {
                i += 1;
            }

            // Try to consume one or more .digits groups
            let mut dot_count = 0;
            while i < len && bytes[i] == b'.' {
                let dot_pos = i;
                i += 1;
                if i < len && bytes[i].is_ascii_digit() {
                    while i < len && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    dot_count += 1;
                } else {
                    i = dot_pos; // backtrack
                    break;
                }
            }

            if dot_count >= 1 {
                for &b in &bytes[start..i] {
                    if b == b'.' {
                        result.push(VERSION_DOT);
                    } else {
                        result.push(b as char);
                    }
                }
            } else {
                // All bytes in this range are ASCII digits, safe to push as char
                for &b in &bytes[start..i] {
                    result.push(b as char);
                }
            }
        } else {
            // Non-digit character: may be multi-byte UTF-8
            let ch = input[i..].chars().next().unwrap();
            result.push(ch);
            i += ch.len_utf8();
        }
    }

    result
}

/// Restore placeholder bytes back to dots after slugification.
fn restore_version_dots(input: &str) -> String {
    input.replace(VERSION_DOT, ".")
}

/// Maximum filename length in bytes (common filesystem limit).
const MAX_FILENAME_BYTES: usize = 255;

/// Maximum slug length in bytes for plain text slugification.
/// Generous ceiling — no real slug should approach this, but it guards
/// against unbounded output if someone feeds in very large input.
const MAX_SLUG_BYTES: usize = 1024;

/// Truncate `base` so that `base + ext` fits within `max_bytes`.
/// Prefers cutting at a separator boundary (dash, underscore) to avoid broken words.
/// Returns the base unchanged if it already fits.
fn truncate_base(base: &str, ext: &str, max_bytes: usize) -> String {
    let budget = max_bytes.saturating_sub(ext.len());
    if base.len() <= budget {
        return base.to_string();
    }

    // Floor to a char boundary to avoid slicing mid-character (keep_unicode mode)
    let budget = (0..=budget)
        .rev()
        .find(|&i| base.is_char_boundary(i))
        .unwrap_or(0);

    // Truncate, then look for the last separator within budget
    let truncated = &base[..budget];
    if let Some(pos) = truncated.rfind(['-', '_']) {
        if pos > 0 {
            return truncated[..pos].to_string();
        }
    }

    // No separator found — hard truncate at budget
    truncated.to_string()
}

/// Slugify a filename according to the given options.
///
/// Converts a filename to a clean, shell-safe slug while preserving its
/// extension, dotfile status, and any embedded version numbers. Returns
/// [`Cow::Borrowed`] when the input is already clean (e.g. dotfiles).
///
/// Names exceeding 255 bytes are silently truncated at a word boundary.
///
/// # Examples
///
/// ```
/// use fileslug::{slugify, SlugifyOptions};
///
/// let opts = SlugifyOptions::default();
/// assert_eq!(slugify("My Résumé (Final).pdf", &opts), "my-resume-final.pdf");
/// assert_eq!(slugify(".gitignore", &opts), ".gitignore");
/// assert_eq!(slugify("app-1.2.3.dmg", &opts), "app-1.2.3.dmg");
/// assert_eq!(slugify("Photo 2024_01.JPG", &opts), "photo-2024-01.JPG");
/// ```
#[must_use]
pub fn slugify<'a>(filename: &'a str, options: &SlugifyOptions) -> Cow<'a, str> {
    if filename.is_empty() {
        return Cow::Borrowed("");
    }

    let (base, ext) = split_extension(filename);

    // Dotfiles with no base: return as-is
    if base.is_empty() {
        return Cow::Borrowed(filename);
    }

    // Remember if original base starts with '.' (dotfile with extension like .env.local)
    let is_dotfile = base.starts_with('.');

    // Step 2: Transliterate
    let base = if options.keep_unicode {
        base.to_string()
    } else {
        any_ascii::any_ascii(base)
    };

    // Step 3: Strip bracket characters, keep contents
    let base = base.replace(['(', ')', '[', ']', '{', '}'], " ");

    // Step 3b: Preserve dots in version numbers (e.g. "0.8.34")
    let base = preserve_version_dots(&base);

    // Step 4: Normalize — collect words (sequences of alphanumeric/unicode chars)
    let words: Vec<String> = if options.keep_unicode {
        base.split(|c: char| !c.is_alphanumeric() && c != VERSION_DOT)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
            .collect()
    } else {
        base.split(|c: char| !c.is_ascii_alphanumeric() && c != VERSION_DOT)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
            .collect()
    };

    if words.is_empty() {
        return Cow::Owned(ext.to_string());
    }

    // Step 5: Join with chosen separator
    let slugified = match options.style {
        Style::Kebab => words.join("-"),
        Style::Snake => words.join("_"),
        Style::Pascal => {
            let mut result = String::new();
            for word in &words {
                let mut chars = word.chars();
                if let Some(first) = chars.next() {
                    result.extend(first.to_uppercase());
                    result.push_str(chars.as_str());
                }
            }
            result
        }
    };

    // Step 5b: Restore version dots
    let slugified = restore_version_dots(&slugified);

    // Step 6: Restore leading dot for dotfiles (e.g. .env.local → .env.local)
    let slugified = if is_dotfile {
        format!(".{slugified}")
    } else {
        slugified
    };

    // Step 7: Truncate if filename would exceed filesystem limit
    let slugified = truncate_base(&slugified, ext, MAX_FILENAME_BYTES);

    // Step 8: Rejoin extension
    Cow::Owned(format!("{slugified}{ext}"))
}

/// Slugify an arbitrary string (not a filename).
///
/// Unlike [`slugify`], this treats the entire input as plain text — no
/// extension splitting, no dotfile preservation. Use this for generating
/// URL slugs, identifiers, or other non-filename use cases.
///
/// # Examples
///
/// ```
/// use fileslug::{slugify_string, SlugifyOptions};
///
/// let opts = SlugifyOptions::default();
/// assert_eq!(slugify_string("My Blog Post Title!", &opts), "my-blog-post-title");
/// assert_eq!(slugify_string("Café Résumé", &opts), "cafe-resume");
/// ```
#[must_use]
pub fn slugify_string(input: &str, options: &SlugifyOptions) -> String {
    if input.is_empty() {
        return String::new();
    }

    // Step 1: Transliterate
    let text = if options.keep_unicode {
        input.to_string()
    } else {
        any_ascii::any_ascii(input)
    };

    // Step 2: Strip bracket characters, keep contents
    let text = text.replace(['(', ')', '[', ']', '{', '}'], " ");

    // Step 3: Preserve dots in version numbers
    let text = preserve_version_dots(&text);

    // Step 4: Normalize — collect words
    let words: Vec<String> = if options.keep_unicode {
        text.split(|c: char| !c.is_alphanumeric() && c != VERSION_DOT)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
            .collect()
    } else {
        text.split(|c: char| !c.is_ascii_alphanumeric() && c != VERSION_DOT)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
            .collect()
    };

    if words.is_empty() {
        return String::new();
    }

    // Step 5: Join with chosen separator
    let slugified = match options.style {
        Style::Kebab => words.join("-"),
        Style::Snake => words.join("_"),
        Style::Pascal => {
            let mut result = String::new();
            for word in &words {
                let mut chars = word.chars();
                if let Some(first) = chars.next() {
                    result.extend(first.to_uppercase());
                    result.push_str(chars.as_str());
                }
            }
            result
        }
    };

    // Step 6: Restore version dots
    let slugified = restore_version_dots(&slugified);

    // Step 7: Truncate to max length
    truncate_base(&slugified, "", MAX_SLUG_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_simple_extension() {
        assert_eq!(split_extension("hello.txt"), ("hello", ".txt"));
    }

    #[test]
    fn test_split_compound_extension() {
        assert_eq!(split_extension("archive.tar.gz"), ("archive", ".tar.gz"));
        assert_eq!(split_extension("backup.tar.bz2"), ("backup", ".tar.bz2"));
        assert_eq!(split_extension("data.tar.xz"), ("data", ".tar.xz"));
        assert_eq!(split_extension("logs.tar.zst"), ("logs", ".tar.zst"));
    }

    #[test]
    fn test_split_dotfile() {
        assert_eq!(split_extension(".gitignore"), ("", ".gitignore"));
        assert_eq!(split_extension(".env"), ("", ".env"));
    }

    #[test]
    fn test_split_no_extension() {
        assert_eq!(split_extension("Makefile"), ("Makefile", ""));
        assert_eq!(split_extension("README"), ("README", ""));
    }

    #[test]
    fn test_split_multiple_dots() {
        assert_eq!(split_extension("my.cool.file.txt"), ("my.cool.file", ".txt"));
    }

    #[test]
    fn test_split_dotfile_with_extension() {
        assert_eq!(split_extension(".bashrc"), ("", ".bashrc"));
    }

    // --- slugify pipeline tests ---

    #[test]
    fn test_slugify_basic_kebab() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify("My Cool File.txt", &opts), "my-cool-file.txt");
    }

    #[test]
    fn test_slugify_snake() {
        let opts = SlugifyOptions { style: Style::Snake, ..Default::default() };
        assert_eq!(slugify("My Cool File.txt", &opts), "my_cool_file.txt");
    }

    #[test]
    fn test_slugify_pascal() {
        let opts = SlugifyOptions { style: Style::Pascal, ..Default::default() };
        assert_eq!(slugify("my cool file.txt", &opts), "MyCoolFile.txt");
    }

    #[test]
    fn test_slugify_unicode_transliterate() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify("Café Résumé.txt", &opts), "cafe-resume.txt");
    }

    #[test]
    fn test_slugify_keep_unicode() {
        let opts = SlugifyOptions { keep_unicode: true, ..Default::default() };
        assert_eq!(slugify("Café Résumé.txt", &opts), "café-résumé.txt");
    }

    #[test]
    fn test_slugify_brackets_stripped_contents_kept() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify("Report (Final) [2024].txt", &opts), "report-final-2024.txt");
    }

    #[test]
    fn test_slugify_compound_extension() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify("My Archive File.tar.gz", &opts), "my-archive-file.tar.gz");
    }

    #[test]
    fn test_slugify_dotfile_untouched() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify(".gitignore", &opts), ".gitignore");
    }

    #[test]
    fn test_slugify_already_clean() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify("already-clean.txt", &opts), "already-clean.txt");
    }

    #[test]
    fn test_slugify_special_chars() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify("file@name#with$symbols.txt", &opts), "file-name-with-symbols.txt");
    }

    #[test]
    fn test_slugify_collapses_separators() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify("too   many   spaces.txt", &opts), "too-many-spaces.txt");
    }

    #[test]
    fn test_slugify_trims_separators() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify(" leading and trailing .txt", &opts), "leading-and-trailing.txt");
    }

    #[test]
    fn test_slugify_no_extension() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify("My Makefile", &opts), "my-makefile");
    }

    #[test]
    fn test_slugify_full_pipeline() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify("Café Résumé (Final Copy) [2024].tar.gz", &opts), "cafe-resume-final-copy-2024.tar.gz");
    }

    #[test]
    fn test_slugify_curly_braces() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify("file {draft}.txt", &opts), "file-draft.txt");
    }

    #[test]
    fn test_slugify_pascal_multiple_words() {
        let opts = SlugifyOptions { style: Style::Pascal, ..Default::default() };
        assert_eq!(slugify("Hello World Foo Bar.txt", &opts), "HelloWorldFooBar.txt");
    }

    #[test]
    fn test_slugify_empty_string() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify("", &opts), "");
    }

    #[test]
    fn test_slugify_only_special_chars() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify("@#$.txt", &opts), ".txt");
    }

    #[test]
    fn test_slugify_numbers_only() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify("12345.txt", &opts), "12345.txt");
    }

    #[test]
    fn test_slugify_dotfile_with_extension() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify(".env.local", &opts), ".env.local");
    }

    #[test]
    fn test_slugify_hidden_config_multipart() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify(".config.backup.old", &opts), ".config-backup.old");
    }

    #[test]
    fn test_split_dotfile_with_second_extension() {
        assert_eq!(split_extension(".env.local"), (".env", ".local"));
    }

    #[test]
    fn test_slugify_only_special_no_ext() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify("@@@", &opts), "");
    }

    // --- Adversarial & unicode tests ---

    #[test]
    fn test_slugify_shell_injection() {
        let opts = SlugifyOptions::default();
        let result = slugify("$(echo pwned).txt", &opts);
        assert!(!result.contains('$'));
        assert!(!result.contains('('));
        assert!(!result.contains(')'));
        assert_eq!(result, "echo-pwned.txt");
    }

    #[test]
    fn test_slugify_backticks() {
        let opts = SlugifyOptions::default();
        let result = slugify("`rm -rf /`.txt", &opts);
        assert!(!result.contains('`'));
        assert_eq!(result, "rm-rf.txt");
    }

    #[test]
    fn test_slugify_pipe_redirect() {
        let opts = SlugifyOptions::default();
        let result = slugify("file|name>output.txt", &opts);
        assert!(!result.contains('|'));
        assert!(!result.contains('>'));
        assert_eq!(result, "file-name-output.txt");
    }

    #[test]
    fn test_slugify_newline_in_name() {
        let opts = SlugifyOptions::default();
        let result = slugify("file\nname.txt", &opts);
        assert!(!result.contains('\n'));
        assert_eq!(result, "file-name.txt");
    }

    #[test]
    fn test_slugify_emoji() {
        let opts = SlugifyOptions::default();
        let result = slugify("🎉.txt", &opts);
        // any_ascii transliterates emoji — result should be ASCII and not empty
        assert!(result.ends_with(".txt"));
        assert!(result.is_ascii());
    }

    #[test]
    fn test_slugify_cjk() {
        let opts = SlugifyOptions::default();
        let result = slugify("你好世界.txt", &opts);
        // any_ascii transliterates CJK to pinyin-like ASCII
        assert!(result.ends_with(".txt"));
        assert!(result.is_ascii());
        assert!(result.len() > ".txt".len(), "CJK should transliterate to something");
    }

    #[test]
    fn test_slugify_rtl_arabic() {
        let opts = SlugifyOptions::default();
        let result = slugify("مرحبا.txt", &opts);
        assert!(result.ends_with(".txt"));
        assert!(result.is_ascii());
        assert!(result.len() > ".txt".len(), "Arabic should transliterate to something");
    }

    #[test]
    fn test_slugify_combining_char() {
        let opts = SlugifyOptions::default();
        // Decomposed é (e + combining acute accent) vs precomposed é
        let decomposed = slugify("caf\u{0065}\u{0301}.txt", &opts);
        let precomposed = slugify("caf\u{00e9}.txt", &opts);
        assert_eq!(decomposed, precomposed);
    }

    #[test]
    fn test_slugify_zero_width_chars() {
        let opts = SlugifyOptions::default();
        // Zero-width space (U+200B) should be stripped
        let result = slugify("hello\u{200B}world.txt", &opts);
        assert!(!result.contains('\u{200B}'));
        assert_eq!(result, "helloworld.txt");
    }

    #[test]
    fn test_slugify_very_long_name() {
        let opts = SlugifyOptions::default();
        let long_name = "a".repeat(255) + ".txt";
        let result = slugify(&long_name, &opts);
        assert!(result.len() <= 255, "result is {} bytes", result.len());
        assert!(result.ends_with(".txt"));
        assert!(!result.is_empty());
    }

    #[test]
    fn test_slugify_already_numbered() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify("file-2.txt", &opts), "file-2.txt");
    }

    #[test]
    fn test_slugify_compound_ext_mixed_case() {
        let opts = SlugifyOptions::default();
        let result = slugify("Archive.TAR.GZ", &opts);
        assert_eq!(result, "archive.TAR.GZ");
    }

    // --- version dot preservation tests ---

    #[test]
    fn test_preserve_version_dots_simple() {
        assert_eq!(preserve_version_dots("foo-0.8.34-bar"), "foo-0\x018\x0134-bar");
    }

    #[test]
    fn test_preserve_version_dots_semver() {
        assert_eq!(preserve_version_dots("app-1.2.3"), "app-1\x012\x013");
    }

    #[test]
    fn test_preserve_version_dots_two_part() {
        assert_eq!(preserve_version_dots("app-7.20"), "app-7\x0120");
    }

    #[test]
    fn test_preserve_version_dots_no_version() {
        assert_eq!(preserve_version_dots("hello-world"), "hello-world");
    }

    #[test]
    fn test_preserve_version_dots_letters_not_matched() {
        assert_eq!(preserve_version_dots("a.b.c"), "a.b.c");
    }

    #[test]
    fn test_preserve_version_dots_not_followed_by_digit() {
        assert_eq!(preserve_version_dots("7.txt"), "7.txt");
    }

    #[test]
    fn test_preserve_version_dots_adjacent_to_letters() {
        assert_eq!(preserve_version_dots("istatmenus7.20"), "istatmenus7\x0120");
    }

    #[test]
    fn test_preserve_version_dots_multiple_versions() {
        assert_eq!(preserve_version_dots("2.10-2.12.26"), "2\x0110-2\x0112\x0126");
    }

    #[test]
    fn test_restore_version_dots() {
        assert_eq!(restore_version_dots("app-1\x012\x013"), "app-1.2.3");
    }

    #[test]
    fn test_restore_version_dots_no_placeholder() {
        assert_eq!(restore_version_dots("hello-world"), "hello-world");
    }

    // --- filename length truncation tests ---

    #[test]
    fn test_slugify_truncates_long_name() {
        let opts = SlugifyOptions::default();
        // 300 'a' chars + .txt → exceeds 255 bytes
        let long_name = "a".repeat(300) + ".txt";
        let result = slugify(&long_name, &opts);
        assert!(result.len() <= 255, "result is {} bytes", result.len());
        assert!(result.ends_with(".txt"));
    }

    #[test]
    fn test_slugify_truncates_at_separator_boundary() {
        let opts = SlugifyOptions::default();
        // Build a long name with words separated by spaces
        // Each word is "abcdefgh" (8 chars) + space, joined produces "abcdefgh-" (9 chars per word)
        // ~30 words = 270 chars + .txt
        let words: Vec<&str> = std::iter::repeat_n("abcdefgh", 30).collect();
        let long_name = words.join(" ") + ".txt";
        let result = slugify(&long_name, &opts);
        assert!(result.len() <= 255, "result is {} bytes", result.len());
        assert!(result.ends_with(".txt"));
        // Should not end with a trailing separator before the extension
        let (base, _ext) = split_extension(&result);
        assert!(!base.ends_with('-'), "should not have trailing separator: {result}");
    }

    #[test]
    fn test_slugify_truncates_cjk_expansion() {
        let opts = SlugifyOptions::default();
        // CJK chars transliterate to multi-char ASCII (e.g. 你 → "Ni ")
        // 200 CJK chars should expand well beyond 255 bytes
        let cjk = "你".repeat(200) + ".txt";
        let result = slugify(&cjk, &opts);
        assert!(result.len() <= 255, "result is {} bytes", result.len());
        assert!(result.ends_with(".txt"));
    }

    #[test]
    fn test_slugify_no_truncation_under_limit() {
        let opts = SlugifyOptions::default();
        let name = "a".repeat(250) + ".txt";
        let result = slugify(&name, &opts);
        // 250 + 4 = 254 bytes, under limit, should not truncate
        assert_eq!(result.len(), 254);
    }

    #[test]
    fn test_slugify_truncation_preserves_long_extension() {
        let opts = SlugifyOptions::default();
        // Long base with .tar.gz extension
        let long_name = "a".repeat(300) + ".tar.gz";
        let result = slugify(&long_name, &opts);
        assert!(result.len() <= 255, "result is {} bytes", result.len());
        assert!(result.ends_with(".tar.gz"));
    }

    #[test]
    fn test_slugify_truncation_keep_unicode_multibyte() {
        let opts = SlugifyOptions { keep_unicode: true, ..Default::default() };
        // 200 × 'é' (2 bytes each) = 400 bytes base + .txt → must truncate without panicking
        let long_name = "é".repeat(200) + ".txt";
        let result = slugify(&long_name, &opts);
        assert!(result.len() <= 255, "result is {} bytes", result.len());
        assert!(result.ends_with(".txt"));
        // Verify it's valid UTF-8 (implicit — it's a &str) and not empty
        assert!(!result.is_empty());
    }

    // --- slugify_string tests ---

    #[test]
    fn test_slugify_string_basic() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify_string("My Blog Post Title!", &opts), "my-blog-post-title");
    }

    #[test]
    fn test_slugify_string_no_extension_handling() {
        let opts = SlugifyOptions::default();
        // Unlike slugify(), dots are not treated as extensions
        assert_eq!(slugify_string("my.blog.post", &opts), "my-blog-post");
    }

    #[test]
    fn test_slugify_string_snake() {
        let opts = SlugifyOptions { style: Style::Snake, ..Default::default() };
        assert_eq!(slugify_string("My Blog Post", &opts), "my_blog_post");
    }

    #[test]
    fn test_slugify_string_pascal() {
        let opts = SlugifyOptions { style: Style::Pascal, ..Default::default() };
        assert_eq!(slugify_string("my blog post", &opts), "MyBlogPost");
    }

    #[test]
    fn test_slugify_string_unicode() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify_string("Café Résumé", &opts), "cafe-resume");
    }

    #[test]
    fn test_slugify_string_keep_unicode() {
        let opts = SlugifyOptions { keep_unicode: true, ..Default::default() };
        assert_eq!(slugify_string("Café Résumé", &opts), "café-résumé");
    }

    #[test]
    fn test_slugify_string_empty() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify_string("", &opts), "");
    }

    #[test]
    fn test_slugify_string_only_special() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify_string("@#$!", &opts), "");
    }

    #[test]
    fn test_slugify_string_preserves_version_dots() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify_string("app version 1.2.3", &opts), "app-version-1.2.3");
    }

    #[test]
    fn test_slugify_string_brackets_stripped() {
        let opts = SlugifyOptions::default();
        assert_eq!(slugify_string("Hello (World) [2024]", &opts), "hello-world-2024");
    }

    #[test]
    fn test_slugify_string_truncates_long_input() {
        let opts = SlugifyOptions::default();
        let long_input = "a ".repeat(600); // 600 words → "a-a-a-..." exceeds 1024 bytes
        let result = slugify_string(&long_input, &opts);
        assert!(result.len() <= 1024, "result is {} bytes", result.len());
        assert!(!result.is_empty());
    }

    #[test]
    fn test_slugify_string_no_truncation_under_1k() {
        let opts = SlugifyOptions::default();
        // 300 words of "ab" → "ab-ab-ab-..." = 899 bytes, above 255 but under 1024
        let input = "ab ".repeat(300);
        let result = slugify_string(&input, &opts);
        assert_eq!(result.len(), 899, "should not truncate under 1024 bytes");
    }

    #[test]
    fn test_slugify_string_dotfile_not_preserved() {
        let opts = SlugifyOptions::default();
        // Unlike slugify(), dotfiles are not treated specially — the dot is a separator
        assert_eq!(slugify_string(".gitignore", &opts), "gitignore");
        assert_eq!(slugify_string(".env.local", &opts), "env-local");
    }
}
