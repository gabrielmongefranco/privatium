// Project:  Privatium™  |  File: crates/privatium/tests/cli.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-04  |  Modified: 2026-09-04
// Summary:  spec/cli.md against the real binary, section by section: the qualified
//           --version (§1) and the exit codes; the flags, which are exactly the spec's
//           synopsis lines (§1–§9, both directions); a node on loopback with --port,
//           --solo and --no-discovery (§2); dev naming the app (§3); new for each tier and
//           from hello (§4); skill list and export (§6); snapshot, --verify, and restore from
//           a backup with its tier reported and a diverged log refused (§7); pair and
//           firewall parsing and refusing (§8, §9); the commands §10 keeps absent.

// AGENTS.md, Style: unwrap() is permitted in tests, and a test that hides a failure
// behind `?` is worse than one that panics with a line number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_privatium");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Run the binary to completion: exit code, stdout, stderr.
fn privatium(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let output = Command::new(BIN)
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// A node started in the background, killed on drop.
struct Running {
    child: Child,
    port: u16,
    stdout: Vec<String>,
}

impl Running {
    /// Start with `args`, wait for the announce line, and keep the port.
    fn start(args: &[&str]) -> Self {
        let mut child = Command::new(BIN)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let mut stdout = Vec::new();
        let port = loop {
            let mut line = String::new();
            let read = reader.read_line(&mut line).unwrap();
            assert_ne!(
                read, 0,
                "the node exited before announcing; stdout so far: {stdout:?}"
            );
            let line = line.trim_end().to_owned();
            stdout.push(line.clone());
            if let Some(rest) = line.strip_prefix("privatium: listening on http://127.0.0.1:") {
                break rest.trim_end_matches('/').parse::<u16>().unwrap();
            }
        };
        // Drain the rest on a thread so the child never blocks on a full pipe.
        let mut running = Self {
            child,
            port,
            stdout,
        };
        // The two lines that follow the announce: the loopback notice, and under `dev`
        // the app's URL.
        for _ in 0..2 {
            let mut line = String::new();
            let started = Instant::now();
            // The dev line is printed right after; the loopback line always. Read what is
            // there without blocking forever on a bare run.
            if reader.read_line(&mut line).unwrap_or(0) > 0 {
                running.stdout.push(line.trim_end().to_owned());
            }
            if started.elapsed() > Duration::from_secs(5) {
                break;
            }
            if running.stdout.iter().any(|l| l.contains(" at http://")) {
                break;
            }
            if !running
                .stdout
                .last()
                .is_some_and(|l| l.contains("loopback only"))
            {
                break;
            }
            if !args.contains(&"dev") {
                break;
            }
        }
        std::thread::spawn(move || {
            let mut sink = String::new();
            let _ = reader.read_to_string(&mut sink);
        });
        running
    }

    /// One request over a fresh connection: status and body.
    fn get(&self, path: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            self.port
        )
        .unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let (head, body) = raw.split_once("\r\n\r\n").unwrap();
        let status: u16 = head.split(' ').nth(1).unwrap().parse().unwrap();
        (status, body.to_owned())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn data_dir(root: &tempfile::TempDir) -> String {
    root.path().join("node").to_string_lossy().into_owned()
}

/// Every `--flag` on the `privatium …` synopsis lines of a text, keyed by the command word
/// (`""` for the bare command).
fn synopsis_flags(text: &str) -> BTreeSet<(String, String)> {
    let mut flags = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("privatium ") else {
            continue;
        };
        let words: Vec<&str> = rest.split_whitespace().collect();
        let command = match words.first() {
            Some(word) if !word.starts_with('[') && !word.starts_with('-') => {
                // `skill list` and `skill export` are two commands.
                match (*word, words.get(1)) {
                    ("skill", Some(sub)) => format!("skill {sub}"),
                    _ => (*word).to_owned(),
                }
            }
            _ => String::new(),
        };
        for word in &words {
            let word = word.trim_matches(|c| c == '[' || c == ']');
            if let Some(flag) = word.strip_prefix("--") {
                let name = flag.split(['=', ' ', '|']).next().unwrap();
                flags.insert((command.clone(), format!("--{name}")));
            }
        }
    }
    flags
}

/// `§1` — `--version` prints the build version and a qualified protocol string, since a
/// Phase 1 build does not satisfy `spec/protocol.md §13` (`docs/plans/phase-1.md §2.1`).
#[test]
fn test_spec_cli_1_version_qualifies_protocol() {
    let root = tempfile::tempdir().unwrap();
    let (code, out, _) = privatium(root.path(), &["--version"]);
    assert_eq!(code, 0);
    assert_eq!(
        out.trim(),
        format!(
            "privatium {} pv/1 (partial: phase 1)",
            env!("CARGO_PKG_VERSION")
        )
    );
    // Terminal wherever it stands.
    let (code, out2, _) = privatium(root.path(), &["dev", "--version"]);
    assert_eq!(code, 0);
    assert_eq!(out2, out);
}

/// `§1` — `0` success, `1` runtime error, `2` usage error. (`3`, lint findings, is M12's.)
#[test]
fn test_cli_exit_codes() {
    let root = tempfile::tempdir().unwrap();
    let dir = data_dir(&root);
    let usage: &[&[&str]] = &[
        &["--nope"],
        &["new"],
        &["new", "_sys"],
        &["new", "Not-A-Slug"],
        &["new", "a", "b"],
        &["new", "a", "--tier", "perl"],
        &["restore"],
        &["skill"],
        &["skill", "export", "no-such-skill"],
        &["--port", "x"],
        &["--data-dir"],
        &["snapshot", "extra"],
    ];
    for args in usage {
        let mut full = vec!["--data-dir", dir.as_str()];
        full.extend_from_slice(args);
        let (code, _, err) = privatium(root.path(), &full);
        assert_eq!(code, 2, "{args:?}: {err}");
        assert!(
            err.contains("privatium ["),
            "{args:?}: the help follows a usage error: {err}"
        );
    }
    let runtime: &[&[&str]] = &[
        &["snapshot", "--app", "no-such-app"],
        &["restore", "--from", "no-such-dir"],
        &["dev", "--app", "no-such-app"],
        &["new", "xy", "--from", "no-such-app"],
        &["lint"],
        &["pair"],
        &["firewall"],
    ];
    for args in runtime {
        let mut full = vec!["--data-dir", dir.as_str()];
        full.extend_from_slice(args);
        let (code, _, err) = privatium(root.path(), &full);
        assert_eq!(code, 1, "{args:?}: {err}");
        assert!(err.starts_with("privatium"), "{args:?}: {err}");
    }
    for args in [&["--version"][..], &["--help"], &["-h"], &["skill", "list"]] {
        let (code, _, _) = privatium(root.path(), args);
        assert_eq!(code, 0, "{args:?}");
    }
}

/// `docs/plans/phase-1.md` M11 — the flags the binary accepts are the flags `spec/cli.md`
/// names, per command, in both directions: nothing undocumented, nothing missing.
#[test]
fn test_no_undocumented_flags() {
    let spec = fs::read_to_string(repo().join("spec").join("cli.md")).unwrap();
    let (code, help, _) = privatium(&repo(), &["--help"]);
    assert_eq!(code, 0);
    let documented = synopsis_flags(&spec);
    let implemented = synopsis_flags(&help);
    assert!(!documented.is_empty());
    let undocumented: Vec<_> = implemented.difference(&documented).collect();
    let missing: Vec<_> = documented.difference(&implemented).collect();
    assert!(
        undocumented.is_empty(),
        "flags not in spec/cli.md: {undocumented:?}"
    );
    assert!(
        missing.is_empty(),
        "spec/cli.md flags the binary lacks: {missing:?}"
    );
    // And the parser really refuses what the help does not list.
    for (command, flag) in [
        ("", "--bind"),
        ("dev", "--port"),
        ("new", "--open"),
        ("snapshot", "--from"),
    ] {
        let mut args = vec![];
        if !command.is_empty() {
            args.push(command);
        }
        args.push(flag);
        args.push("x");
        let (code, _, err) = privatium(&repo(), &args);
        assert_eq!(code, 2, "{command} {flag}: {err}");
    }
}

/// `§2` — bare `privatium` runs a node on loopback: `--port`, `--solo` for one run, and
/// `--no-discovery` a notice until Phase 2 (`docs/plans/phase-1.md §2.1`).
#[test]
fn test_spec_cli_2_runs_a_node_on_loopback() {
    let root = tempfile::tempdir().unwrap();
    let dir = data_dir(&root);

    let node = Running::start(&["--data-dir", &dir, "--port", "0"]);
    assert!(
        node.stdout
            .iter()
            .any(|l| l.contains("loopback only — LAN access arrives with pairing")),
        "{:?}",
        node.stdout
    );
    let (status, body) = node.get("/api/v1/health");
    assert_eq!(status, 200, "{body}");
    let health: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(health["v"], 1);
    assert!(health["id"].is_string());
    let (status, body) = node.get("/");
    assert_eq!(status, 200);
    assert!(
        body.contains("Hello"),
        "the launcher lists the reference apps: {body}"
    );
    let (status, body) = node.get("/a/hello/");
    assert_eq!(status, 200);
    assert!(body.contains("We haven't met yet."), "{body}");
    drop(node);

    // The state file was written on the way down and the port never touched config.toml.
    assert!(Path::new(&dir).join("local").join("state.jsonl").is_file());
    assert!(!Path::new(&dir).join("config.toml").exists());

    // --solo: one app at `/`, no launcher, for this run only.
    let solo = Running::start(&[
        "--data-dir",
        &dir,
        "--port",
        "0",
        "--solo",
        "hello",
        "--no-discovery",
    ]);
    let (status, body) = solo.get("/");
    assert_eq!(status, 200);
    assert!(body.contains("We haven't met yet."), "{body}");
    let (status, _) = solo.get("/a/hello/");
    assert_eq!(status, 404);
    drop(solo);

    // --no-discovery's notice goes to stderr; capture it with a short-lived run.
    let mut child = Command::new(BIN)
        .args(["--data-dir", &dir, "--port", "0", "--no-discovery"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    child.kill().unwrap();
    let output = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("--no-discovery: there is no discovery to disable"),
        "{err}"
    );
}

/// `§3` — `dev --app <slug>` runs the node and names the app: where its files are, and
/// its URL. An app that did not load is a runtime error naming why.
#[test]
fn test_spec_cli_3_dev_names_the_app() {
    let root = tempfile::tempdir().unwrap();
    let dir = data_dir(&root);
    fs::create_dir_all(&dir).unwrap();
    fs::write(Path::new(&dir).join("config.toml"), "[node]\nport = 0\n").unwrap();

    let dev = Running::start(&["--data-dir", &dir, "dev", "--app", "hello"]);
    assert!(
        dev.stdout
            .iter()
            .any(|l| l.starts_with("privatium: hello at http://127.0.0.1:")
                && l.ends_with("/a/hello/")),
        "{:?}",
        dev.stdout
    );
    let (status, body) = dev.get("/a/hello/edit");
    assert_eq!(status, 200);
    assert!(body.contains("What should I call you?"), "{body}");
    drop(dev);

    let (code, _, err) = privatium(root.path(), &["--data-dir", &dir, "dev", "--app", "nope"]);
    assert_eq!(code, 1);
    assert!(err.contains("dev --app nope"), "{err}");
}

/// `§4` — `new` writes an app under `<data-dir>/apps/<slug>/` for each tier, refuses a
/// second time, and `--scaffold` adds screens to a folder that already has a schema.
#[test]
fn test_spec_cli_4_new_each_tier_loads() {
    let root = tempfile::tempdir().unwrap();
    let dir = data_dir(&root);
    let apps = Path::new(&dir).join("apps");

    let (code, out, err) = privatium(root.path(), &["--data-dir", &dir, "new", "my-app"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("my-app/app.toml"), "{out}");
    assert!(apps.join("my-app").join("app.lua").is_file());
    assert!(
        apps.join("my-app")
            .join("views")
            .join("index.lsp")
            .is_file()
    );
    assert!(err.contains("privatium dev --app my-app"), "{err}");
    let manifest = fs::read_to_string(apps.join("my-app").join("app.toml")).unwrap();
    assert!(manifest.contains("slug        = \"my-app\""), "{manifest}");
    assert!(manifest.contains("title       = \"My App\""), "{manifest}");

    let (code, _, _) = privatium(
        root.path(),
        &["--data-dir", &dir, "new", "my-web", "--tier", "web"],
    );
    assert_eq!(code, 0);
    assert!(apps.join("my-web").join("web").join("index.html").is_file());
    let (code, _, _) = privatium(
        root.path(),
        &["--data-dir", &dir, "new", "my-rust", "--tier=rust"],
    );
    assert_eq!(code, 0);
    assert!(apps.join("my-rust").join("README.md").is_file());
    assert!(!apps.join("my-rust").join("app.lua").exists());

    // Never overwrites.
    let (code, _, err) = privatium(root.path(), &["--data-dir", &dir, "new", "my-app"]);
    assert_eq!(code, 1);
    assert!(err.contains("already exists"), "{err}");

    // --scaffold against an existing folder's schema.sql, refusing to replace app.lua.
    fs::write(
        apps.join("my-app").join("schema.sql"),
        "CREATE TABLE note (id VARCHAR PRIMARY KEY, text VARCHAR NOT NULL);",
    )
    .unwrap();
    let (code, _, err) = privatium(
        root.path(),
        &["--data-dir", &dir, "new", "my-app", "--scaffold", "note"],
    );
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("app.lua") && err.contains("never overwrites"),
        "{err}"
    );
    fs::remove_file(apps.join("my-app").join("app.lua")).unwrap();
    let (code, out, err) = privatium(
        root.path(),
        &["--data-dir", &dir, "new", "my-app", "--scaffold", "note"],
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("my-app/views/note_form.lsp"), "{out}");
    let (code, _, err) = privatium(
        root.path(),
        &["--data-dir", &dir, "new", "my-app", "--scaffold", "nope"],
    );
    assert_eq!(code, 1);
    assert!(err.contains("declares no table \"nope\""), "{err}");

    // Every one of them loads on the node.
    let node = Running::start(&["--data-dir", &dir, "--port", "0"]);
    for (path, needle) in [
        ("/a/my-app/", "<h1>Note</h1>"),
        ("/a/my-web/", "<h1>My Web</h1>"),
        ("/", "My Rust"),
    ] {
        let (status, body) = node.get(path);
        assert_eq!(status, 200, "{path}");
        assert!(body.contains(needle), "{path}: {body}");
    }
}

/// `§4` — `--from hello` copies the reference app and rewrites its slug and title, and the
/// copy runs beside the original.
#[test]
fn test_new_from_hello_rewrites_slug_and_title() {
    let root = tempfile::tempdir().unwrap();
    let dir = data_dir(&root);
    let app = Path::new(&dir).join("apps").join("greeter");

    let (code, out, err) = privatium(
        root.path(),
        &["--data-dir", &dir, "new", "greeter", "--from", "hello"],
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("greeter/views/edit.lsp"), "{out}");
    let manifest = fs::read_to_string(app.join("app.toml")).unwrap();
    assert!(manifest.contains("slug        = \"greeter\""), "{manifest}");
    assert!(manifest.contains("title       = \"Greeter\""), "{manifest}");
    assert!(manifest.contains("apps/greeter/app.toml"), "{manifest}");
    let lua = fs::read_to_string(app.join("app.lua")).unwrap();
    assert!(lua.contains("File: apps/greeter/app.lua"), "{lua}");
    assert!(!lua.contains("apps/hello/"), "{lua}");
    let skill = fs::read_to_string(app.join("SKILL.md")).unwrap();
    assert!(skill.contains("name: privatium-app-greeter"), "{skill}");

    // A tier that disagrees with the copy is a usage error; a copy over a folder is refused.
    let (code, _, err) = privatium(
        root.path(),
        &[
            "--data-dir",
            &dir,
            "new",
            "other",
            "--from",
            "hello",
            "--tier",
            "web",
        ],
    );
    assert_eq!(code, 2, "{err}");
    let (code, _, _) = privatium(
        root.path(),
        &["--data-dir", &dir, "new", "greeter", "--from", "hello"],
    );
    assert_eq!(code, 1);

    // Copy plus scaffold: hello's schema, the scaffold's screens.
    let (code, out, err) = privatium(
        root.path(),
        &[
            "--data-dir",
            &dir,
            "new",
            "profiles",
            "--from",
            "hello",
            "--scaffold",
            "profile",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("profiles/views/profile_index.lsp"), "{out}");
    let lua = fs::read_to_string(Path::new(&dir).join("apps/profiles/app.lua")).unwrap();
    assert!(lua.contains("--scaffold profile"), "{lua}");

    let node = Running::start(&["--data-dir", &dir, "--port", "0"]);
    let (status, body) = node.get("/a/greeter/");
    assert_eq!(status, 200);
    assert!(body.contains("We haven't met yet."), "{body}");
    assert!(body.contains("<title>Greeter"), "{body}");
    let (status, body) = node.get("/a/hello/");
    assert_eq!(status, 200, "{body}");
    let (status, body) = node.get("/a/profiles/");
    assert_eq!(status, 200);
    assert!(body.contains("<h1>Profile</h1>"), "{body}");
}

/// `§6` — `skill list` names every skill this build ships and `skill export` writes the
/// tree the repository holds, byte for byte, matching the running version.
#[test]
fn test_spec_cli_6_skill_list_and_export() {
    let root = tempfile::tempdir().unwrap();
    let (code, out, _) = privatium(root.path(), &["skill", "list"]);
    assert_eq!(code, 0);
    let names: Vec<&str> = out.lines().filter(|l| !l.starts_with(' ')).collect();
    assert_eq!(
        names,
        [
            "privatium-accessibility",
            "privatium-games",
            "privatium-overview",
            "privatium-security",
            "privatium-tier1-lua",
            "privatium-tier2-web",
            "privatium-tier3-rust",
        ]
    );
    assert!(out.contains("    Start here"), "descriptions follow: {out}");

    // Everything, into the default `skills/` under the working directory.
    let (code, _, err) = privatium(root.path(), &["skill", "export"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("pv/1 (partial: phase 1)"), "{err}");
    let exported = root.path().join("skills");
    let source = repo().join("skills");
    let mut count = 0;
    for entry in walk(&source) {
        let relative = entry.strip_prefix(&source).unwrap();
        let copy = exported.join(relative);
        assert!(copy.is_file(), "{}", relative.display());
        assert_eq!(
            fs::read(&entry).unwrap(),
            fs::read(&copy).unwrap(),
            "{}",
            relative.display()
        );
        count += 1;
    }
    assert_eq!(walk(&exported).len(), count);

    // One skill, into --out.
    let out_dir = root.path().join("one");
    let (code, _, _) = privatium(
        root.path(),
        &[
            "skill",
            "export",
            "privatium-tier1-lua",
            "--out",
            out_dir.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0);
    assert!(
        out_dir
            .join("privatium-tier1-lua")
            .join("SKILL.md")
            .is_file()
    );
    assert!(
        out_dir
            .join("privatium-tier1-lua")
            .join("reference")
            .join("README.md")
            .is_file()
    );
    assert!(!out_dir.join("README.md").exists());
    assert!(!out_dir.join("privatium-overview").exists());
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files.sort();
    files
}

/// `§7` — `snapshot` writes the set of `spec/protocol.md §5`; `--verify` recomputes the
/// checksums and exits non-zero on a mismatch, writing nothing.
#[test]
fn test_spec_cli_7_snapshot_and_verify() {
    let root = tempfile::tempdir().unwrap();
    let dir = data_dir(&root);
    let (code, out, err) = privatium(root.path(), &["--data-dir", &dir, "snapshot"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("_sys: 20"), "{out}");
    assert!(out.contains("hello: 20"), "every loaded app: {out}");
    let snap = Path::new(&dir).join("data").join("_sys").join("snap");
    let ids: Vec<PathBuf> = fs::read_dir(&snap)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(ids.len(), 1);
    assert!(ids[0].join("MANIFEST.json").is_file());
    assert!(ids[0].join("sys_device.csv").is_file());

    let (code, out, _) = privatium(root.path(), &["--data-dir", &dir, "snapshot", "--verify"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.lines()
            .any(|l| l.starts_with("_sys: ") && l.ends_with(": ok")),
        "{out}"
    );
    assert_eq!(
        fs::read_dir(&snap).unwrap().count(),
        1,
        "--verify writes no snapshot"
    );

    // One app only, and a flipped byte.
    let csv = ids[0].join("sys_device.csv");
    let mut bytes = fs::read(&csv).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xFF;
    fs::write(&csv, bytes).unwrap();
    let (code, out, err) = privatium(
        root.path(),
        &["--data-dir", &dir, "snapshot", "--verify", "--app", "_sys"],
    );
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("sys_device: csv mismatch"), "{out}");
    assert!(out.contains("MISMATCH"), "{out}");
    assert!(err.contains("do not match MANIFEST.json"), "{err}");
    assert!(!out.contains("hello:"), "--app narrows: {out}");
}

/// `§7` — `restore --from <backup>` brings a `data/` folder in, rebuilds each app by the
/// three tiers and reports the tier; `--dry-run` reports and copies nothing.
#[test]
fn test_spec_cli_7_restore_from_backup_reports_tier() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a").to_string_lossy().into_owned();
    let b = root.path().join("b").to_string_lossy().into_owned();

    // Node A: a hello event through the app, then a snapshot of everything.
    let node = Running::start(&["--data-dir", &a, "--port", "0"]);
    drop(node);
    let a_log_dir = Path::new(&a).join("data").join("hello").join("log");
    let a_log = fs::read_dir(&a_log_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let dev = a_log.file_stem().unwrap().to_string_lossy().into_owned();
    fs::write(
        &a_log,
        format!(
            "{{\"seq\":1,\"lam\":1,\"ts\":\"2026-09-04T00:00:00.000Z\",\"dev\":\"{dev}\",\"app\":\"hello\",\"op\":\"put\",\"tbl\":\"profile\",\"id\":\"01K4B0000000000000000000AA\",\"d\":{{\"display_name\":\"Backed Up\"}}}}\n"
        ),
    )
    .unwrap();
    let (code, _, err) = privatium(
        root.path(),
        &["--data-dir", &a, "snapshot", "--app", "hello"],
    );
    assert_eq!(code, 0, "{err}");
    let backup = root.path().join("backup");
    copy_dir(&Path::new(&a).join("data"), &backup.join("data"));

    // Node B, fresh: dry run first.
    let (code, out, err) = privatium(
        root.path(),
        &[
            "--data-dir",
            &b,
            "restore",
            "--from",
            backup.to_str().unwrap(),
            "--dry-run",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains(&format!("copy  hello/log/{dev}.jsonl (absent here)")),
        "{out}"
    );
    assert!(out.contains("copy  hello/snap/"), "{out}");
    assert!(out.contains("dry run: nothing copied"), "{out}");
    assert!(
        !Path::new(&b)
            .join("data")
            .join("hello")
            .join("log")
            .join(format!("{dev}.jsonl"))
            .exists()
    );

    // The real thing: hello's cache comes from the snapshot, tier 1.
    let (code, out, err) = privatium(
        root.path(),
        &[
            "--data-dir",
            &b,
            "restore",
            "--from",
            backup.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("hello: used tier 1 (sqlite) from 20"), "{out}");
    assert!(
        Path::new(&b)
            .join("data")
            .join("hello")
            .join("log")
            .join(format!("{dev}.jsonl"))
            .is_file()
    );
    let node = Running::start(&["--data-dir", &b, "--port", "0"]);
    let (status, body) = node.get("/a/hello/");
    assert_eq!(status, 200);
    assert!(body.contains("Backed Up"), "{body}");
    drop(node);

    // The same backup again is all "keep"; one narrowed to an app that is not in it is nothing.
    let (code, out, _) = privatium(
        root.path(),
        &[
            "--data-dir",
            &b,
            "restore",
            "--from",
            backup.to_str().unwrap(),
            "--app",
            "hello",
        ],
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains(&format!("keep  hello/log/{dev}.jsonl (identical)")),
        "{out}"
    );
    assert!(!out.contains("copy  "), "{out}");
    let (code, out, _) = privatium(
        root.path(),
        &[
            "--data-dir",
            &b,
            "restore",
            "--from",
            backup.to_str().unwrap(),
            "--app",
            "nope",
        ],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("nothing for nope"), "{out}");
}

/// `§7`, `spec/protocol.md §3.1` — a backup whose copy of a log has gone a different way
/// from this node's is refused whole, before anything is written.
#[test]
fn test_spec_cli_7_restore_refuses_a_diverged_log() {
    let root = tempfile::tempdir().unwrap();
    let dir = data_dir(&root);
    let node = Running::start(&["--data-dir", &dir, "--port", "0"]);
    drop(node);
    let backup = root.path().join("backup");
    copy_dir(&Path::new(&dir).join("data"), &backup);
    // Alter the backup's copy of this node's own _sys log, and add a snapshot dir to it,
    // so there is something the plan would otherwise copy.
    let log_dir = backup.join("_sys").join("log");
    let log = fs::read_dir(&log_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut text = fs::read_to_string(&log).unwrap();
    text.replace_range(text.len() - 3.., "X\n");
    fs::write(&log, text).unwrap();
    fs::create_dir_all(backup.join("_sys").join("snap").join("2026-W36-zzzz-1")).unwrap();
    fs::write(backup.join("_sys/snap/2026-W36-zzzz-1/MANIFEST.json"), "{}").unwrap();

    let (code, out, err) = privatium(
        root.path(),
        &[
            "--data-dir",
            &dir,
            "restore",
            "--from",
            backup.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 1, "{out}\n{err}");
    assert!(out.contains("DIVERGED  _sys/log/"), "{out}");
    assert!(err.contains("nothing was written"), "{err}");
    assert!(
        !Path::new(&dir)
            .join("data/_sys/snap/2026-W36-zzzz-1")
            .exists()
    );
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// `§8`, `§9` — `pair` and `firewall` parse their flags and say they are not in this
/// build, so the help text is the spec's without pretending to a phase that is not here.
#[test]
fn test_spec_cli_8_9_pair_and_firewall_parse_and_refuse() {
    let root = tempfile::tempdir().unwrap();
    let (code, _, err) = privatium(root.path(), &["pair", "--open", "--timeout", "30"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("privatium pair: not in this build") && err.contains("Phase 2"),
        "{err}"
    );
    let (code, _, err) = privatium(root.path(), &["firewall", "--apply"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("privatium firewall: not in this build"),
        "{err}"
    );
    let (code, _, err) = privatium(root.path(), &["lint", ".", "--format", "json"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("privatium lint: not in this build") && err.contains("M12"),
        "{err}"
    );
    // A wrong flag is still a usage error, not a "not in this build".
    let (code, _, _) = privatium(root.path(), &["pair", "--qr"]);
    assert_eq!(code, 2);
}

/// `§10` — what is deliberately absent stays absent: neither a command nor a help entry.
#[test]
fn test_spec_cli_10_absent_commands() {
    let root = tempfile::tempdir().unwrap();
    let (_, help, _) = privatium(root.path(), &["--help"]);
    for absent in [
        "doctor", "diagnose", "serve", "migrate", "install", "login", "account",
    ] {
        let (code, _, err) = privatium(root.path(), &[absent]);
        assert_eq!(code, 2, "{absent}");
        assert!(err.contains("spec/cli.md §10"), "{absent}: {err}");
        assert!(
            !help.contains(&format!("privatium {absent}")),
            "{absent} in help"
        );
    }
}
