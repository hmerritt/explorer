use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use tokei::{Config, Languages};

const CODEBASE_IGNORE_PATTERNS: &[&str] = &[".git", "target", "vendor"];
pub(super) const GITHUB_LANGUAGE_FALLBACK_COLOR: u32 = 0xe8e8e8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CodebaseSummary {
    pub(super) repo_root: PathBuf,
    pub(super) total_code: usize,
    pub(super) languages: Vec<CodebaseLanguageSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CodebaseLanguageSummary {
    pub(super) name: String,
    pub(super) code: usize,
    pub(super) percentage: usize,
    pub(super) color: u32,
}

pub(super) fn scan_codebase_summary(path: &Path) -> Option<CodebaseSummary> {
    let repo_root = find_git_repository_root(path)?;
    let (total_code, languages) = language_summary_for_repository(&repo_root);
    Some(CodebaseSummary {
        repo_root,
        total_code,
        languages,
    })
}

pub(super) fn find_git_repository_root(path: &Path) -> Option<PathBuf> {
    let mut current = path;
    loop {
        if current.join(".git").is_dir() {
            return Some(current.to_path_buf());
        }

        current = current.parent()?;
    }
}

fn language_summary_for_repository(repo_root: &Path) -> (usize, Vec<CodebaseLanguageSummary>) {
    let mut languages = Languages::new();
    let config = Config::default();
    languages.get_statistics(&[repo_root], CODEBASE_IGNORE_PATTERNS, &config);

    let total_code = languages
        .iter()
        .map(|(_, language)| language.code)
        .sum::<usize>();
    let summaries = language_summaries(
        languages
            .iter()
            .map(|(language_type, language)| (language_type.to_string(), language.code)),
        total_code,
    );

    (total_code, summaries)
}

fn language_summaries(
    languages: impl IntoIterator<Item = (String, usize)>,
    total_code: usize,
) -> Vec<CodebaseLanguageSummary> {
    if total_code == 0 {
        return Vec::new();
    }

    let mut grouped_languages = BTreeMap::new();
    for (name, code) in languages.into_iter().filter(|(_, code)| *code > 0) {
        let name = codebase_language_display_name(&name);
        *grouped_languages.entry(name).or_insert(0) += code;
    }

    let mut summaries = grouped_languages
        .into_iter()
        .map(|(name, code)| {
            let exact_percentage = (code as f64 / total_code as f64) * 100.0;
            CodebaseLanguageSummary {
                color: github_language_color(&name),
                name,
                code,
                percentage: exact_percentage.round() as usize,
            }
        })
        .collect::<Vec<_>>();

    summaries.sort_by(|left, right| {
        right
            .code
            .cmp(&left.code)
            .then_with(|| left.name.cmp(&right.name))
    });
    summaries
}

fn codebase_language_display_name(name: &str) -> String {
    languages::from_name(name)
        .map(|language| language.group.unwrap_or(language.name).to_owned())
        .unwrap_or_else(|| name.to_owned())
}

pub(super) fn language_segment_widths(
    languages: &[CodebaseLanguageSummary],
    total_code: usize,
    bar_width: f32,
) -> Vec<f32> {
    if languages.is_empty() {
        return Vec::new();
    }

    if total_code == 0 || bar_width <= 0.0 {
        return vec![0.0; languages.len()];
    }

    let bar_width = bar_width.round() as usize;
    if bar_width == 0 {
        return vec![0.0; languages.len()];
    }

    let mut widths = languages
        .iter()
        .map(|language| {
            let exact_width = (language.code as f64 / total_code as f64) * bar_width as f64;
            (exact_width.floor() as usize, exact_width.fract())
        })
        .collect::<Vec<_>>();
    let allocated = widths.iter().map(|(width, _)| *width).sum::<usize>();
    let mut remaining = bar_width.saturating_sub(allocated);
    let mut order = widths
        .iter()
        .enumerate()
        .map(|(ix, (_, remainder))| (ix, *remainder))
        .collect::<Vec<_>>();
    order.sort_by(|(left_ix, left), (right_ix, right)| {
        right.total_cmp(left).then_with(|| left_ix.cmp(right_ix))
    });

    for (ix, _) in order {
        if remaining == 0 {
            break;
        }
        widths[ix].0 += 1;
        remaining -= 1;
    }

    widths.into_iter().map(|(width, _)| width as f32).collect()
}

fn github_language_color(name: &str) -> u32 {
    if name.eq_ignore_ascii_case("Other") {
        return GITHUB_LANGUAGE_FALLBACK_COLOR;
    }

    languages::from_name(name)
        .and_then(|language| language.color)
        .and_then(parse_github_language_color)
        .unwrap_or(GITHUB_LANGUAGE_FALLBACK_COLOR)
}

fn parse_github_language_color(color: &str) -> Option<u32> {
    let hex = color.strip_prefix('#')?;
    (hex.len() == 6)
        .then(|| u32::from_str_radix(hex, 16).ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;

    #[test]
    fn git_root_detection_accepts_current_repository_root() {
        let temp = TempDir::new();
        std::fs::create_dir(temp.path().join(".git")).expect("create git dir");

        assert_eq!(
            find_git_repository_root(temp.path()),
            Some(temp.path().to_path_buf())
        );
    }

    #[test]
    fn git_root_detection_walks_up_from_nested_folder() {
        let temp = TempDir::new();
        let nested = temp.path().join("src").join("nested");
        std::fs::create_dir(temp.path().join(".git")).expect("create git dir");
        std::fs::create_dir_all(&nested).expect("create nested folders");

        assert_eq!(
            find_git_repository_root(&nested),
            Some(temp.path().to_path_buf())
        );
    }

    #[test]
    fn git_root_detection_returns_none_outside_repository() {
        let temp = TempDir::new();

        assert_eq!(find_git_repository_root(temp.path()), None);
    }

    #[test]
    fn language_summary_keeps_total_sorted_languages_and_percentages() {
        let summaries = language_summaries(
            [
                ("Rust".to_owned(), 18_240),
                ("TOML".to_owned(), 390),
                ("Markdown".to_owned(), 150),
            ],
            18_780,
        );

        assert_eq!(
            summaries
                .iter()
                .map(|summary| (summary.name.as_str(), summary.code, summary.percentage))
                .collect::<Vec<_>>(),
            vec![("Rust", 18_240, 97), ("TOML", 390, 2), ("Markdown", 150, 1)]
        );
    }

    #[test]
    fn language_summary_groups_linguist_children_under_parent() {
        let summaries = language_summaries(
            [
                ("TSX".to_owned(), 5_848),
                ("TypeScript".to_owned(), 4_852),
                ("JSON".to_owned(), 300),
            ],
            11_000,
        );

        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].name, "TypeScript");
        assert_eq!(summaries[0].code, 10_700);
        assert_eq!(summaries[0].percentage, 97);
        assert_eq!(summaries[0].color, 0x3178c6);
        assert!(!summaries.iter().any(|summary| summary.name == "TSX"));
    }

    #[test]
    fn language_summary_includes_nonzero_languages_below_one_percent() {
        let summaries = language_summaries(
            [
                ("Rust".to_owned(), 990),
                ("TOML".to_owned(), 9),
                ("Markdown".to_owned(), 1),
            ],
            1_000,
        );

        assert_eq!(
            summaries
                .iter()
                .map(|summary| (summary.name.as_str(), summary.code, summary.percentage))
                .collect::<Vec<_>>(),
            vec![("Rust", 990, 99), ("TOML", 9, 1), ("Markdown", 1, 0)]
        );
    }

    #[test]
    fn language_summary_hides_zero_loc_results() {
        let summaries = language_summaries([("Rust".to_owned(), 0)], 0);

        assert!(summaries.is_empty());
    }

    #[test]
    fn github_language_color_uses_linguist_color_and_fallback() {
        assert_eq!(github_language_color("Rust"), 0xdea584);
        assert_eq!(github_language_color("Other"), 0xe8e8e8);
        assert_eq!(github_language_color("Not A GitHub Language"), 0xe8e8e8);
    }

    #[test]
    fn github_language_color_parser_accepts_rrggbb_hex_only() {
        assert_eq!(parse_github_language_color("#dea584"), Some(0xdea584));
        assert_eq!(parse_github_language_color("dea584"), None);
        assert_eq!(parse_github_language_color("#dea58"), None);
        assert_eq!(parse_github_language_color("#nothex"), None);
    }

    #[test]
    fn language_segment_widths_fill_bar_with_whole_pixels_and_preserve_dominant_language() {
        let summaries = language_summaries(
            [
                ("Rust".to_owned(), 84),
                ("TOML".to_owned(), 10),
                ("Markdown".to_owned(), 6),
            ],
            100,
        );

        let widths = language_segment_widths(&summaries, 100, 101.0);
        let total_width = widths.iter().sum::<f32>();

        assert_eq!(summaries[0].name, "Rust");
        assert!(widths[0] > widths[1]);
        assert_eq!(total_width, 101.0);
        assert!(widths.iter().all(|width| width.fract() == 0.0));
    }

    #[test]
    fn repository_scan_uses_gitignore_and_vendor_ignores() {
        let temp = TempDir::new();
        std::fs::create_dir(temp.path().join(".git")).expect("create git dir");
        std::fs::create_dir(temp.path().join("src")).expect("create src dir");
        std::fs::create_dir(temp.path().join("vendor")).expect("create vendor dir");
        std::fs::write(temp.path().join(".gitignore"), "ignored.rs\n").expect("write gitignore");
        std::fs::write(temp.path().join("src").join("main.rs"), "fn main() {}\n")
            .expect("write source");
        std::fs::write(temp.path().join("ignored.rs"), "fn ignored() {}\n")
            .expect("write ignored source");
        std::fs::write(
            temp.path().join("vendor").join("vendored.rs"),
            "fn vendored() {}\n",
        )
        .expect("write vendored source");

        let summary = scan_codebase_summary(temp.path()).expect("codebase summary");

        assert_eq!(summary.total_code, 1);
        assert_eq!(
            summary
                .languages
                .iter()
                .map(|summary| (summary.name.as_str(), summary.code, summary.percentage))
                .collect::<Vec<_>>(),
            vec![("Rust", 1, 100)]
        );
    }
}
