//! Keeps every test repository from signing commits with the developer's key.
//!
//! A developer with `commit.gpgsign = true` in their global git config would
//! otherwise run their signing program (`ssh-keygen` or `gpg`) for every commit
//! a fixture makes, and for every commit the code under test makes inside a
//! fixture, which inherits the test process's environment and so the global
//! config. That is slow, can prompt for a passphrase, and uses a real key for
//! throwaway objects.
//!
//! The fix lives in the repositories rather than in the process: every fixture
//! repository is created from [`FIXTURE_TEMPLATE_DIR`], whose `config` sets
//! `commit.gpgsign` and `tag.gpgsign` to false. Repository config outranks the
//! global file, so the setting holds for every git command run inside the
//! fixture, a production one included, without changing how production code
//! builds its git commands and without a workspace-wide environment variable
//! (which would also reach `cargo run`).
//!
//! [`fixture_git`] is the git command a test uses to build and drive a fixture.
//! It differs from a plain `git` only by `GIT_TEMPLATE_DIR`, which git reads
//! solely when it creates a repository (`init`, and `clone` for the copy), so
//! it is safe to use for every fixture command, not just the creating one.
//!
//! Compiled only for dux-core's own tests and for the `test-support` feature, so
//! none of it ships.

use std::process::Command;

/// The template directory fixture repositories are created from. It holds only
/// a `config` file, which git copies into the new repository.
pub const FIXTURE_TEMPLATE_DIR: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/git-template");

/// A `git` command for building or driving a test fixture repository. Any
/// repository it creates never signs, whatever the global config says.
pub fn fixture_git() -> Command {
    let mut command = Command::new("git");
    keep_fixture_unsigned(&mut command);
    command
}

/// Applies the fixture template to an already-built `git` command, for helpers
/// that build their command some other way (see `git::test_support`).
pub fn keep_fixture_unsigned(command: &mut Command) -> &mut Command {
    command.env("GIT_TEMPLATE_DIR", FIXTURE_TEMPLATE_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A global config that signs every commit and tag with a program that does
    /// not exist, so a commit that tries to sign fails loudly instead of
    /// reaching for anybody's key.
    fn signing_global(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("global-gitconfig");
        std::fs::write(
            &path,
            "[commit]\n\tgpgsign = true\n[tag]\n\tgpgsign = true\n\
             [gpg]\n\tformat = ssh\n\
             [gpg \"ssh\"]\n\tprogram = /nonexistent/dux-test-signer\n\
             [user]\n\tsigningkey = /nonexistent/dux-test-key\n\
             \tname = t\n\temail = t@t\n",
        )
        .unwrap();
        path
    }

    /// Point one git invocation at the simulated global config, and at nothing
    /// else, so the developer's own files cannot decide the answer either way.
    fn with_global<'a>(command: &'a mut Command, global: &Path) -> &'a mut Command {
        command
            .env("GIT_CONFIG_GLOBAL", global)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
    }

    fn run(command: &mut Command) -> std::process::Output {
        let out = command.output().expect("run git");
        assert!(
            out.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    /// What `key` resolves to inside `repo` when the global config signs.
    fn resolved(repo: &Path, global: &Path, key: &str) -> String {
        let mut command = Command::new("git");
        command.arg("-C").arg(repo).args(["config", "--get", key]);
        let out = run(with_global(&mut command, global));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Try one empty commit inside `repo` the way production code would: a plain
    /// `git`, inheriting the (simulated) global config.
    fn production_commit(repo: &Path, global: &Path) -> std::process::Output {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(repo)
            .args(["commit", "-q", "--allow-empty", "-m", "c"]);
        with_global(&mut command, global).output().unwrap()
    }

    #[test]
    fn the_template_directory_exists_and_holds_the_config() {
        let config = Path::new(FIXTURE_TEMPLATE_DIR).join("config");
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.contains("gpgsign = false"), "{text}");
        assert!(text.contains("[maintenance]\n\tauto = false"), "{text}");
    }

    #[test]
    fn a_fixture_repository_never_signs_even_when_the_global_config_does() {
        let tmp = tempfile::tempdir().unwrap();
        let global = signing_global(tmp.path());

        let repo = tmp.path().join("repo");
        run(with_global(
            fixture_git().args(["init", "-q", "-b", "main"]).arg(&repo),
            &global,
        ));
        let bare = tmp.path().join("bare.git");
        run(with_global(
            fixture_git()
                .args(["init", "-q", "--bare", "-b", "main"])
                .arg(&bare),
            &global,
        ));
        let clone = tmp.path().join("clone");
        run(with_global(
            fixture_git().arg("clone").arg("-q").arg(&bare).arg(&clone),
            &global,
        ));

        for created in [&repo, &bare, &clone] {
            assert_eq!(resolved(created, &global, "commit.gpgsign"), "false");
            assert_eq!(resolved(created, &global, "tag.gpgsign"), "false");
        }
        for work_tree in [&repo, &clone] {
            let out = production_commit(work_tree, &global);
            assert!(
                out.status.success(),
                "a commit in a fixture repository tried to sign: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    /// The control that proves the simulation bites: the same global config
    /// makes a repository created WITHOUT the fixture template sign, and its
    /// commit fails on the missing signer.
    #[test]
    fn a_repository_made_without_the_template_signs_under_the_same_global() {
        let tmp = tempfile::tempdir().unwrap();
        let global = signing_global(tmp.path());
        let repo = tmp.path().join("repo");
        let mut init = Command::new("git");
        init.env_remove("GIT_TEMPLATE_DIR")
            .args(["init", "-q", "-b", "main"])
            .arg(&repo);
        run(with_global(&mut init, &global));

        assert_eq!(resolved(&repo, &global, "commit.gpgsign"), "true");
        assert!(
            !production_commit(&repo, &global).status.success(),
            "the control commit should have tried to sign and failed"
        );
    }
}
