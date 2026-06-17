use std::path::{Path, PathBuf};

use thousands::Separable;
use tokei::{Config, Languages};

const CODEBASE_IGNORE_PATTERNS: &[&str] = &[".git", "target", "vendor"];
const MIN_LANGUAGE_PERCENTAGE: f64 = 1.0;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CodebaseSummary {
    pub(super) repo_root: PathBuf,
    pub(super) text: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CodebaseLanguageSummary {
    name: String,
    code: usize,
    percentage: usize,
}

pub(super) fn scan_codebase_summary(path: &Path) -> Option<CodebaseSummary> {
    let repo_root = find_git_repository_root(path)?;
    let text = language_summary_for_repository(&repo_root);
    Some(CodebaseSummary { repo_root, text })
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

fn language_summary_for_repository(repo_root: &Path) -> Option<String> {
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

    format_language_summaries(&summaries)
}

fn language_summaries(
    languages: impl IntoIterator<Item = (String, usize)>,
    total_code: usize,
) -> Vec<CodebaseLanguageSummary> {
    if total_code == 0 {
        return Vec::new();
    }

    let mut summaries = languages
        .into_iter()
        .filter(|(_, code)| *code > 0)
        .filter_map(|(name, code)| {
            let exact_percentage = (code as f64 / total_code as f64) * 100.0;
            (exact_percentage >= MIN_LANGUAGE_PERCENTAGE).then(|| CodebaseLanguageSummary {
                name,
                code,
                percentage: exact_percentage.round() as usize,
            })
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

fn format_language_summaries(summaries: &[CodebaseLanguageSummary]) -> Option<String> {
    if summaries.is_empty() {
        return None;
    }

    Some(
        summaries
            .iter()
            .map(|summary| {
                format!(
                    "{} {}% ({} LoC)",
                    summary.name,
                    summary.percentage,
                    summary.code.separate_with_commas()
                )
            })
            .collect::<Vec<_>>()
            .join("  "),
    )
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
    fn language_summary_formats_percentages_and_loc() {
        let summaries = language_summaries(
            [
                ("Rust".to_owned(), 18_240),
                ("TOML".to_owned(), 390),
                ("Markdown".to_owned(), 150),
            ],
            18_780,
        );

        assert_eq!(
            format_language_summaries(&summaries),
            Some("Rust 97% (18,240 LoC)  TOML 2% (390 LoC)".to_owned())
        );
    }

    #[test]
    fn language_summary_omits_languages_below_one_percent() {
        let summaries = language_summaries(
            [
                ("Rust".to_owned(), 990),
                ("TOML".to_owned(), 9),
                ("Markdown".to_owned(), 1),
            ],
            1_000,
        );

        assert_eq!(
            summaries,
            vec![CodebaseLanguageSummary {
                name: "Rust".to_owned(),
                code: 990,
                percentage: 99,
            }]
        );
    }

    #[test]
    fn language_summary_hides_zero_loc_results() {
        let summaries = language_summaries([("Rust".to_owned(), 0)], 0);

        assert!(summaries.is_empty());
        assert_eq!(format_language_summaries(&summaries), None);
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

        assert_eq!(summary.text, Some("Rust 100% (1 LoC)".to_owned()));
    }
}
