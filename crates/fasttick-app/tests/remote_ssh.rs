//! Remote file operations against a real OpenSSH server (see `sshd/`).
//! The tests skip if OpenSSH is not installed; CI sets
//! `FASTTICK_REQUIRE_SSHD=1`.

mod sshd;

use std::path::PathBuf;
use std::time::Duration;

use fasttick_app::remote::askpass::{AskpassServer, PromptKind};
use fasttick_app::remote::ops::{self, FetchProblem, Upload};
use fasttick_app::remote::ssh::{self, SshError};
use fasttick_app::remote::{NO_FILE, posix_join};
use sshd::{HOST, HOST_ASK, HOST_BAD_KEY, PASSPHRASE, TestSshd};

fn text(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn list_fetch_upload_and_conflicts() {
    let Some(server) = TestSshd::start() else {
        return;
    };
    let files = server.files();
    let trees = files.join("trees");
    let lib = files.join("lib");
    std::fs::create_dir_all(trees.join("sub")).unwrap();
    std::fs::create_dir_all(&lib).unwrap();
    let lib_file = lib.join("abs.xml");
    std::fs::write(&lib_file, r#"<root BTCPP_format="4"/>"#).unwrap();
    let main_text = format!(
        r#"<root BTCPP_format="4">
  <include path="sub/a.xml"/>
  <include path="{}"/>
  <BehaviorTree ID="Main"><AlwaysSuccess/></BehaviorTree>
</root>
"#,
        lib_file.display()
    );
    let main = trees.join("main.xml");
    std::fs::write(&main, &main_text).unwrap();
    std::fs::write(trees.join("sub/a.xml"), r#"<root BTCPP_format="4"/>"#).unwrap();
    std::fs::write(trees.join("other.xml"), r#"<root BTCPP_format="4"/>"#).unwrap();
    std::fs::write(trees.join("notes.txt"), "not a tree").unwrap();
    std::fs::write(trees.join(".hidden.xml"), "hidden").unwrap();
    let odd = trees.join("it's a $file.xml");
    std::fs::write(&odd, "<root/>").unwrap();

    let config = server.ssh_config("main", None);
    let host = server.host(HOST);
    let trees_path = trees.to_str().unwrap().to_owned();

    // List.
    let listing = ops::list_dir(&config, &host, &trees_path).unwrap();
    assert_eq!(
        listing.dir,
        std::fs::canonicalize(&trees).unwrap().to_str().unwrap()
    );
    let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "sub",
            "it's a $file.xml",
            "main.xml",
            "notes.txt",
            "other.xml"
        ]
    );
    assert!(listing.entries[0].is_dir);
    let home = ops::list_dir(&config, &host, "").unwrap();
    assert!(home.dir.starts_with('/'));
    let error = ops::list_dir(&config, &host, "/no/such/folder").unwrap_err();
    assert!(error.to_string().contains("does not exist"), "{error}");

    // Open: the file, the tree files of its folder and the includes.
    let main_path = main.to_str().unwrap().to_owned();
    let collected = ops::collect(&config, &host, &main_path).unwrap();
    assert_eq!(collected[0].path, main_path);
    assert_eq!(collected[0].content.as_deref(), Ok(main_text.as_bytes()));
    let paths: Vec<&str> = collected.iter().map(|f| f.path.as_str()).collect();
    for wanted in [
        posix_join(&trees_path, "other.xml"),
        posix_join(&trees_path, "sub/a.xml"),
        posix_join(&trees_path, "it's a $file.xml"),
        lib_file.to_str().unwrap().to_owned(),
    ] {
        assert!(
            paths.contains(&wanted.as_str()),
            "{wanted} not in {paths:?}"
        );
    }
    assert!(!paths.iter().any(|p| p.ends_with("notes.txt")), "{paths:?}");
    let base = collected[0].base.clone();
    assert_ne!(base, NO_FILE);

    // A missing file and a file over the size limit.
    let fetched = ops::fetch(
        &config,
        &host,
        &[posix_join(&trees_path, "gone.xml"), main_path.clone()],
        10,
    )
    .unwrap();
    assert_eq!(fetched[0].content, Err(FetchProblem::Missing));
    assert!(matches!(fetched[1].content, Err(FetchProblem::TooLarge(_))));

    // Upload: the file keeps its permissions.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&main, std::fs::Permissions::from_mode(0o640)).unwrap();
    }
    let new_text = main_text.replace("Main", "Changed");
    let result = ops::upload(&config, &host, &main_path, new_text.as_bytes(), Some(&base)).unwrap();
    let Upload::Done { base: new_base } = result else {
        panic!("{result:?}");
    };
    assert_ne!(new_base, base);
    assert_eq!(text(&main), new_text);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&main).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }
    // No temporary files stay in the folder.
    let leftovers: Vec<String> = std::fs::read_dir(&trees)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("fasttick"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // The old checksum: somebody changed the file, nothing is written.
    let result = ops::upload(&config, &host, &main_path, b"lost", Some(&base)).unwrap();
    assert_eq!(
        result,
        Upload::Conflict {
            current: new_base.clone()
        }
    );
    assert_eq!(text(&main), new_text);
    // Overwrite without the check.
    let result = ops::upload(&config, &host, &main_path, main_text.as_bytes(), None).unwrap();
    assert!(matches!(result, Upload::Done { .. }));
    assert_eq!(text(&main), main_text);

    // A new file in a new folder; a file that must not exist yet.
    let new_path = posix_join(&trees_path, "new/dir/t.xml");
    let result = ops::upload(&config, &host, &new_path, b"<root/>", Some(NO_FILE)).unwrap();
    assert!(matches!(result, Upload::Done { .. }));
    assert_eq!(text(&trees.join("new/dir/t.xml")), "<root/>");
    let result = ops::upload(&config, &host, &new_path, b"x", Some(NO_FILE)).unwrap();
    assert!(matches!(result, Upload::Conflict { .. }));

    // A link stays a link; the file that it points to changes.
    #[cfg(unix)]
    {
        let link = trees.join("link.xml");
        std::os::unix::fs::symlink(&main, &link).unwrap();
        let link_path = link.to_str().unwrap();
        ops::upload(&config, &host, link_path, b"<root>via link</root>", None).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(text(&main), "<root>via link</root>");
    }

    // Quoting: a name with a quote and a dollar sign.
    let odd_path = odd.to_str().unwrap();
    let fetched = ops::fetch(&config, &host, &[odd_path.to_owned()], ops::MAX_FILE).unwrap();
    assert_eq!(fetched[0].content.as_deref(), Ok(&b"<root/>"[..]));
    ops::upload(
        &config,
        &host,
        odd_path,
        b"<root>odd</root>",
        Some(&fetched[0].base),
    )
    .unwrap();
    assert_eq!(text(&odd), "<root>odd</root>");

    // A binary file to a local file.
    let bytes: Vec<u8> = (0..=255u8).cycle().take(300_000).collect();
    let log = files.join("run.btlog");
    std::fs::write(&log, &bytes).unwrap();
    let local = tempfile::tempdir().unwrap();
    let copy = local.path().join("copy.btlog");
    ops::fetch_to_file(&config, &host, log.to_str().unwrap(), &copy).unwrap();
    assert_eq!(std::fs::read(&copy).unwrap(), bytes);
    let error = ops::fetch_to_file(&config, &host, "/no/such.btlog", &copy.with_file_name("x"))
        .unwrap_err();
    assert!(matches!(error, SshError::Command { .. }), "{error:?}");
    assert!(!copy.with_file_name("x").exists());

    // A command of the user.
    let output = ops::run_command(&config, &host, "echo started; echo note >&2").unwrap();
    assert!(
        output.contains("started") && output.contains("note"),
        "{output}"
    );
    let error = ops::run_command(&config, &host, "echo why >&2; exit 3").unwrap_err();
    assert_eq!(
        error,
        SshError::Command {
            code: Some(3),
            message: "why".into()
        }
    );
    ssh::close_master(&config, &host);
}

#[test]
fn the_dialog_answers_host_key_and_passphrase_questions() {
    let Some(server) = TestSshd::start() else {
        return;
    };
    let askpass = AskpassServer::start(|| {}).unwrap();
    let program = PathBuf::from(env!("CARGO_BIN_EXE_fasttick"));
    let config = server.ssh_config("ask", Some(askpass.env(program.clone())));
    let host = server.host(HOST_ASK);
    let dir = server.files().to_str().unwrap().to_owned();

    // Cancel the host key question: no connection.
    let cancelled = {
        let (config, host, dir) = (config.clone(), host.clone(), dir.clone());
        std::thread::spawn(move || ops::list_dir(&config, &host, &dir))
    };
    let prompt = askpass
        .wait_prompt(Duration::from_secs(20))
        .expect("a question");
    assert_eq!(prompt.kind(), PromptKind::Confirm, "{}", prompt.text);
    prompt.answer(None);
    let error = cancelled.join().unwrap().unwrap_err();
    assert!(matches!(error, SshError::Connect(_)), "{error:?}");

    // Accept the host key, then give the passphrase of the key.
    let listed = {
        let (config, host, dir) = (config.clone(), host.clone(), dir.clone());
        std::thread::spawn(move || ops::list_dir(&config, &host, &dir))
    };
    let prompt = askpass
        .wait_prompt(Duration::from_secs(20))
        .expect("a question");
    assert_eq!(prompt.kind(), PromptKind::Confirm, "{}", prompt.text);
    prompt.answer(Some("yes".into()));
    let prompt = askpass
        .wait_prompt(Duration::from_secs(20))
        .expect("a question");
    assert_eq!(prompt.kind(), PromptKind::Secret, "{}", prompt.text);
    assert!(prompt.text.contains("passphrase"), "{}", prompt.text);
    prompt.answer(Some(PASSPHRASE.into()));
    let listing = listed.join().unwrap().unwrap();
    assert!(listing.dir.ends_with("robot"));

    // The next operations use the shared connection: no more questions.
    ops::list_dir(&config, &host, &dir).unwrap();
    assert!(askpass.wait_prompt(Duration::from_millis(300)).is_none());
    ssh::close_master(&config, &host);
}

#[test]
fn a_key_that_the_server_refuses_is_a_connection_error() {
    let Some(server) = TestSshd::start() else {
        return;
    };
    let config = server.ssh_config("bad", None);
    let error = ops::list_dir(&config, &server.host(HOST_BAD_KEY), "").unwrap_err();
    let SshError::Connect(message) = &error else {
        panic!("{error:?}");
    };
    assert!(message.contains("Permission denied"), "{message}");
    assert!(error.to_string().starts_with("FastTick cannot connect"));
}
