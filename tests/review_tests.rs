use assert_cmd::Command;
use git2::{build::CheckoutBuilder, IndexAddOption, Oid, Repository, Signature};
use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const SECRET: &str = "sk-abcdefghijklmnopqrstuvwxyz12345";

fn write_base_files(root: &Path) {
    fs::create_dir_all(root.join("src")).expect("src directory");
    fs::create_dir_all(root.join("tests")).expect("tests directory");
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"review-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("Cargo.toml");
    fs::write(
        root.join("src/lib.rs"),
        "pub fn refresh_token(input: &str) -> String {\n    format!(\"old:{input}\")\n}\n",
    )
    .expect("lib.rs");
    fs::write(
        root.join("src/main.rs"),
        "mod lib;\n\nfn main() {\n    let _ = lib::refresh_token(\"demo\");\n}\n",
    )
    .expect("main.rs");
    fs::write(
        root.join("tests/auth_test.rs"),
        "// refresh_token is covered by this test\n#[test]\nfn refresh_token_works() {}\n",
    )
    .expect("test fixture");
    fs::write(
        root.join("README.md"),
        "# Review fixture\n\nThe src/lib.rs refresh_token path is documented here.\n",
    )
    .expect("README");
}

fn commit_all(repo: &Repository, message: &str) -> Oid {
    let mut index = repo.index().expect("index");
    index.add_all(["*"].iter(), IndexAddOption::DEFAULT, None).expect("add files");
    index.write().expect("write index");
    let tree_id = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_id).expect("tree");
    let signature = Signature::now("review tests", "review@example.invalid").expect("signature");
    let parent = repo
        .head()
        .ok()
        .and_then(|head| head.target())
        .map(|oid| repo.find_commit(oid).expect("parent"));
    let parents = parent.as_ref().map(|commit| vec![commit]).unwrap_or_default();
    repo.commit(Some("HEAD"), &signature, &signature, message, &tree, &parents).expect("commit")
}

fn fixture_repo() -> (TempDir, Repository, Oid) {
    let temp = TempDir::new().expect("temporary repository");
    write_base_files(temp.path());
    let repo = Repository::init(temp.path()).expect("init repository");
    let base = commit_all(&repo, "base");
    (temp, repo, base)
}

fn checkout_detached(repo: &Repository, commit: Oid) {
    let object = repo.find_object(commit, None).expect("checkout object");
    let mut options = CheckoutBuilder::new();
    options.safe();
    repo.checkout_tree(&object, Some(&mut options)).expect("checkout target tree");
    repo.set_head_detached(commit).expect("detach HEAD");
}

fn review_command(root: &Path) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    command.current_dir(root).args(["review", "--path"]).arg(root);
    command
}

fn changed_file(json: &Value) -> &Value {
    json["changed_files"]
        .as_array()
        .expect("changed files")
        .iter()
        .find(|file| file["path"] == "src/lib.rs")
        .expect("changed lib.rs")
}

#[test]
fn working_tree_review_is_deterministic_explainable_and_redacted() {
    let (temp, _repo, _base) = fixture_repo();
    fs::write(
        temp.path().join("src/lib.rs"),
        format!(
            "pub fn refresh_token(input: &str) -> String {{\n    let api_key = \"{SECRET}\";\n    format!(\"new:{{api_key}}:{{input}}\")\n}}\n"
        ),
    )
    .expect("modify lib.rs");

    let first = review_command(temp.path())
        .args(["--working-tree", "--format", "text"])
        .output()
        .expect("first review");
    assert!(first.status.success(), "{:?}", first);
    let second = review_command(temp.path())
        .args(["--working-tree", "--format", "text"])
        .output()
        .expect("second review");
    assert!(second.status.success(), "{:?}", second);
    assert_eq!(first.stdout, second.stdout);

    let json_output = review_command(temp.path())
        .args(["--working-tree", "--format", "json"])
        .output()
        .expect("json review");
    assert!(json_output.status.success(), "{:?}", json_output);
    let json: Value = serde_json::from_slice(&json_output.stdout).expect("ImpactPackV1 JSON");
    assert_eq!(json["schema"], "ImpactPackV1");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["comparison"]["mode"], "working_tree");

    let changed = changed_file(&json);
    assert_eq!(changed["status"], "modified");
    assert!(changed["symbols"]
        .as_array()
        .expect("symbols")
        .iter()
        .any(|symbol| symbol["name"] == "refresh_token"));
    assert!(changed["snippets"].as_array().expect("snippets").iter().any(|snippet| snippet
        ["content"]
        .as_str()
        .is_some_and(|content| {
            !content.contains(SECRET)
                && (content.contains("[REDACTED") || content.contains("[HIGH_ENTROPY_REDACTED"))
        })));

    let related = json["related_files"].as_array().expect("related files");
    assert!(related.iter().any(|file| {
        file["path"] == "src/main.rs"
            && file["relation"] == "caller"
            && file["reason"].as_str().is_some_and(|reason| !reason.is_empty())
    }));
    assert!(related
        .iter()
        .any(|file| file["path"] == "tests/auth_test.rs" && file["relation"] == "test"));
    assert!(related
        .iter()
        .any(|file| file["path"] == "Cargo.toml" && file["relation"] == "config"));
    assert!(related
        .iter()
        .any(|file| file["path"] == "README.md" && file["relation"] == "documentation"));
}

#[test]
fn ref_review_writes_versioned_json_and_keeps_output_stable() {
    let (temp, repo, base) = fixture_repo();
    fs::write(
        temp.path().join("src/lib.rs"),
        "pub fn refresh_token(input: &str) -> String {\n    format!(\"head:{input}\")\n}\n",
    )
    .expect("modify lib.rs");
    let head = commit_all(&repo, "head");
    let output_dir = TempDir::new().expect("output directory");
    let output = output_dir.path().join("impact-pack.json");

    review_command(temp.path())
        .args([
            "--base",
            &base.to_string(),
            "--head",
            &head.to_string(),
            "--format",
            "both",
            "--output",
            output.to_str().expect("output path"),
            "--max-related-files",
            "3",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("Impact pack: ImpactPackV1"));

    let first = fs::read_to_string(&output).expect("impact pack");
    let second_output = output_dir.path().join("impact-pack-2.json");
    review_command(temp.path())
        .args([
            "--base",
            &base.to_string(),
            "--head",
            &head.to_string(),
            "--format",
            "json",
            "--output",
            second_output.to_str().expect("second output path"),
            "--max-related-files",
            "3",
        ])
        .assert()
        .success();
    let second = fs::read_to_string(second_output).expect("second impact pack");
    assert_eq!(first, second);

    let json: Value = serde_json::from_str(&first).expect("valid impact pack");
    assert_eq!(json["schema"], "ImpactPackV1");
    assert_eq!(json["comparison"]["mode"], "refs");
    assert_eq!(json["limits"]["max_related_files"], 3);
    assert!(json["related_files"].as_array().expect("related files").len() <= 3);
}

#[test]
fn ref_review_marks_bounded_diff_sections_as_truncated() {
    let (temp, repo, _base) = fixture_repo();
    let old_source = (0..=2_100)
        .map(|index| format!("pub fn generated_{index}() -> usize {{ {index} }}\n"))
        .collect::<String>();
    fs::write(temp.path().join("src/lib.rs"), old_source).expect("large base source");
    let large_base = commit_all(&repo, "large base");

    let new_source = (0..=2_100)
        .map(|index| format!("pub fn generated_{index}() -> usize {{ {} }}\n", index + 1))
        .collect::<String>();
    fs::write(temp.path().join("src/lib.rs"), new_source).expect("large head source");
    let head = commit_all(&repo, "large head");
    let output_dir = TempDir::new().expect("output directory");
    let output = output_dir.path().join("impact-pack.json");

    review_command(temp.path())
        .args([
            "--base",
            &large_base.to_string(),
            "--head",
            &head.to_string(),
            "--format",
            "json",
            "--output",
            output.to_str().expect("output path"),
        ])
        .assert()
        .success();

    let json: Value = serde_json::from_str(&fs::read_to_string(output).expect("impact pack"))
        .expect("valid impact pack");
    let changed = changed_file(&json);
    assert_eq!(changed["changed_lines"].as_array().expect("changed lines").len(), 256);
    assert_eq!(changed["snippets"].as_array().expect("snippets").len(), 48);
    assert_eq!(changed["symbols"].as_array().expect("symbols").len(), 2_048);
    assert_eq!(json["limits"]["truncated"], true);
}

#[test]
fn ref_review_rejects_unrelated_dirty_worktree_with_actionable_error() {
    let (temp, repo, base) = fixture_repo();
    fs::write(
        temp.path().join("src/lib.rs"),
        "pub fn refresh_token(input: &str) -> String {\n    format!(\"head:{input}\")\n}\n",
    )
    .expect("modify lib.rs");
    let head = commit_all(&repo, "head");
    fs::write(temp.path().join("README.md"), "unrelated local note\n").expect("dirty README");

    review_command(temp.path())
        .args(["--base", &base.to_string(), "--head", &head.to_string(), "--format", "json"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("clean worktree"))
        .stderr(predicates::str::contains("--working-tree"));
}

#[test]
fn ref_review_rejects_clean_checkout_that_does_not_match_requested_head() {
    let (temp, repo, base) = fixture_repo();
    fs::write(
        temp.path().join("src/lib.rs"),
        "pub fn refresh_token(input: &str) -> String {\n    format!(\"head:{input}\")\n}\n",
    )
    .expect("modify lib.rs");
    let requested_head = commit_all(&repo, "requested head");
    checkout_detached(&repo, base);

    review_command(temp.path())
        .args([
            "--base",
            &base.to_string(),
            "--head",
            &requested_head.to_string(),
            "--format",
            "json",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("checked-out HEAD"))
        .stderr(predicates::str::contains("match requested --head"))
        .stderr(predicates::str::contains("Check out the requested head commit"))
        .stderr(predicates::str::contains("--working-tree"));
}

#[test]
fn review_rejects_invalid_format_and_conflicting_comparison_modes() {
    let (temp, _repo, _base) = fixture_repo();
    review_command(temp.path())
        .args(["--format", "yaml"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("Invalid review format"));
    review_command(temp.path()).args(["--head", "HEAD", "--working-tree"]).assert().failure();
}
