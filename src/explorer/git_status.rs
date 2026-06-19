use std::path::{Path, PathBuf};

use git2::{BranchType, Reference, Repository};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct GitRepositoryStatus {
    pub(super) repo_root: PathBuf,
    pub(super) branch: String,
    pub(super) divergence: Option<GitDivergence>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct GitDivergence {
    pub(super) outgoing: usize,
    pub(super) incoming: usize,
}

pub(super) fn scan_git_repository_status(path: &Path) -> Option<GitRepositoryStatus> {
    let repo = Repository::discover(path).ok()?;
    let repo_root = repo.workdir()?.to_path_buf();
    let head = repo.head().ok()?;
    let branch = head_label(&head)?;
    let divergence = branch_divergence(&repo, &head);

    Some(GitRepositoryStatus {
        repo_root,
        branch,
        divergence,
    })
}

fn head_label(head: &Reference<'_>) -> Option<String> {
    if head.is_branch() {
        return head.shorthand().map(ToOwned::to_owned);
    }

    head.target()
        .map(|oid| format!("detached {}", short_oid(oid)))
        .or_else(|| head.shorthand().map(ToOwned::to_owned))
}

fn short_oid(oid: git2::Oid) -> String {
    oid.to_string().chars().take(7).collect()
}

fn branch_divergence(repo: &Repository, head: &Reference<'_>) -> Option<GitDivergence> {
    let branch_name = head.shorthand()?;
    let branch = repo.find_branch(branch_name, BranchType::Local).ok()?;
    let upstream = branch.upstream().ok()?;
    let local_oid = branch.get().target()?;
    let upstream_oid = upstream.get().target()?;
    let (outgoing, incoming) = repo.graph_ahead_behind(local_oid, upstream_oid).ok()?;

    Some(GitDivergence { outgoing, incoming })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;
    use git2::{Commit, Oid, Signature};

    #[test]
    fn git_status_returns_none_outside_repository() {
        let temp = TempDir::new();

        assert_eq!(scan_git_repository_status(temp.path()), None);
    }

    #[test]
    fn git_status_reports_branch_without_upstream() {
        let temp = TempDir::new();
        let repo = init_test_repo(temp.path());
        commit_on_ref(&repo, Some("HEAD"), "file.txt", "initial", "initial", &[]);

        assert_eq!(
            scan_git_repository_status(temp.path()),
            Some(GitRepositoryStatus {
                repo_root: temp.path().to_path_buf(),
                branch: "main".to_owned(),
                divergence: None,
            })
        );
    }

    #[test]
    fn git_status_walks_up_from_nested_folder() {
        let temp = TempDir::new();
        let nested = temp.path().join("src").join("nested");
        std::fs::create_dir_all(&nested).expect("create nested folders");
        let repo = init_test_repo(temp.path());
        commit_on_ref(&repo, Some("HEAD"), "file.txt", "initial", "initial", &[]);

        let status = scan_git_repository_status(&nested).expect("git status");

        assert_eq!(status.repo_root, temp.path());
        assert_eq!(status.branch, "main");
    }

    #[test]
    fn git_status_reports_local_upstream_divergence() {
        let temp = TempDir::new();
        let repo = init_test_repo(temp.path());
        repo.remote("origin", "https://example.invalid/repo.git")
            .expect("create remote");
        let initial_oid = commit_on_ref(&repo, Some("HEAD"), "file.txt", "initial", "initial", &[]);
        repo.reference(
            "refs/remotes/origin/main",
            initial_oid,
            true,
            "create origin/main",
        )
        .expect("create upstream ref");
        repo.find_branch("main", BranchType::Local)
            .expect("find local branch")
            .set_upstream(Some("origin/main"))
            .expect("set upstream");

        let initial = repo.find_commit(initial_oid).expect("find initial commit");
        commit_on_ref(
            &repo,
            Some("HEAD"),
            "file.txt",
            "local",
            "local",
            &[&initial],
        );
        commit_on_ref(
            &repo,
            Some("refs/remotes/origin/main"),
            "file.txt",
            "remote",
            "remote",
            &[&initial],
        );
        drop(initial);

        assert_eq!(
            scan_git_repository_status(temp.path()),
            Some(GitRepositoryStatus {
                repo_root: temp.path().to_path_buf(),
                branch: "main".to_owned(),
                divergence: Some(GitDivergence {
                    outgoing: 1,
                    incoming: 1,
                }),
            })
        );
    }

    fn init_test_repo(path: &Path) -> Repository {
        let repo = Repository::init(path).expect("init repo");
        repo.set_head("refs/heads/main").expect("set HEAD branch");
        repo
    }

    fn commit_on_ref(
        repo: &Repository,
        update_ref: Option<&str>,
        file_name: &str,
        content: &str,
        message: &str,
        parents: &[&Commit<'_>],
    ) -> Oid {
        let signature =
            Signature::now("Explorer Tests", "explorer@example.com").expect("create signature");
        let parent_tree = parents.first().and_then(|parent| parent.tree().ok());
        let mut builder = repo
            .treebuilder(parent_tree.as_ref())
            .expect("create tree builder");
        let blob = repo.blob(content.as_bytes()).expect("write blob");
        builder
            .insert(file_name, blob, 0o100644)
            .expect("insert tree entry");
        let tree_oid = builder.write().expect("write tree");
        let tree = repo.find_tree(tree_oid).expect("find tree");

        repo.commit(update_ref, &signature, &signature, message, &tree, parents)
            .expect("commit")
    }
}
