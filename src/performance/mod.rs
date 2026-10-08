//! Opt-in, local performance experiments. None of this module is built into releases.
pub mod fixtures;
pub mod report;
mod runner;
pub(crate) mod ui;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const SCENARIO_VERSION: u32 = 1;
pub const CONFIG_ENV: &str = "EXPLORER_BENCH_CONFIG_ROOT";

pub(crate) fn config_root() -> Option<&'static Path> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        std::env::var_os(CONFIG_ENV).map(|value| {
            let root = PathBuf::from(value);
            assert!(
                root.is_absolute(),
                "benchmark configuration root must be absolute"
            );
            root
        })
    })
    .as_deref()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Scenario {
    pub id: String,
    pub flow: String,
    pub entries: usize,
    pub view: String,
    pub cache: String,
    pub video: bool,
}

pub fn scenarios(full: bool) -> Vec<Scenario> {
    let mut result = Vec::new();
    let sizes: &[usize] = if full {
        &[0, 100, 1_000, 10_000]
    } else {
        &[1_000]
    };
    for &entries in sizes {
        for view in ["details", "large_icons"] {
            let mut flows = if entries == 0 {
                vec!["startup", "refresh"]
            } else {
                vec!["startup", "open", "back", "forward", "up", "refresh"]
            };
            if entries > 0 {
                flows.extend([
                    "filter",
                    "recursive_search",
                    "single_selection",
                    "range_selection",
                    "select_all",
                    "new_tab",
                    "switch_tab",
                    "close_tab",
                    "scroll_down",
                    "scroll_up",
                ]);
                // Headers are a Details-view interaction.
                if view == "details" {
                    flows.extend([
                        "sort_name_asc",
                        "sort_name_desc",
                        "sort_date_asc",
                        "sort_date_desc",
                        "sort_type_asc",
                        "sort_type_desc",
                        "sort_size_asc",
                        "sort_size_desc",
                    ]);
                }
            }
            for flow in flows {
                result.push(Scenario {
                    id: format!("ui/{flow}/{view}/{entries}"),
                    flow: flow.into(),
                    entries,
                    view: view.into(),
                    cache: "fresh_process".into(),
                    video: false,
                });
            }
        }
    }
    for view in ["details", "large_icons"] {
        for flow in [
            "image_viewer",
            "hover_image",
            "hover_video",
            "hover_text",
            "hover_pdf",
            "hover_epub",
            "image_thumbnails",
            "video_thumbnails",
        ] {
            if view == "details" && flow.ends_with("thumbnails") {
                continue;
            }
            for cache in if full {
                &["empty", "primed"][..]
            } else {
                &["empty"][..]
            } {
                result.push(Scenario {
                    id: format!("ui/{flow}/{view}/{cache}"),
                    flow: flow.into(),
                    entries: 1_000,
                    view: view.into(),
                    cache: (*cache).into(),
                    video: flow.contains("video"),
                });
            }
        }
    }
    result
}

pub fn main() -> Result<(), String> {
    runner::main()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scenario_ids_are_unique_and_quick_is_a_subset() {
        let full = scenarios(true);
        let ids = full
            .iter()
            .map(|s| &s.id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), full.len());
        assert!(scenarios(false).iter().all(|s| ids.contains(&s.id)));
        assert!(full.iter().any(|s| s.entries == 0));
        assert!(
            !full
                .iter()
                .any(|s| s.view == "large_icons" && s.flow.starts_with("sort_"))
        );
    }

    #[test]
    fn isolated_configuration_uses_one_root_for_all_resolvers() {
        if std::env::var_os("EXPLORER_BENCH_ISOLATION_CHILD").is_some() {
            let root = config_root().expect("isolated child has a root");
            assert_eq!(crate::settings::config_dir().as_deref(), Some(root));
            for platform in [
                crate::settings::ConfigPlatform::Windows,
                crate::settings::ConfigPlatform::MacOS,
                crate::settings::ConfigPlatform::Linux,
            ] {
                assert_eq!(
                    crate::settings::config_dir_for(platform, |_| panic!(
                        "must not resolve personal environment paths"
                    ))
                    .as_deref(),
                    Some(root)
                );
            }
            assert_eq!(
                crate::settings::settings_path().unwrap(),
                root.join("settings.json")
            );
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let personal = temp.path().join("personal.json");
        std::fs::write(&personal, b"personal state sentinel").unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "performance::tests::isolated_configuration_uses_one_root_for_all_resolvers",
            ])
            .env(CONFIG_ENV, temp.path().join("isolated"))
            .env("EXPLORER_BENCH_ISOLATION_CHILD", "1")
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(std::fs::read(personal).unwrap(), b"personal state sentinel");
    }
}
