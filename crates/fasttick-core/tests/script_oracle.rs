//! Compares the script check with BT.CPP 4.9.0. `script_oracle.tsv` has
//! the result of `BT::ValidateScript` for about 3,700 scripts: hand-written
//! cases, random token strings and random expressions. The messages of
//! BT.CPP can contain single bytes of a UTF-8 character, so the test
//! compares them after a lossy UTF-8 conversion.

use fasttick_core::script::check_script;

fn unhex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
        .collect()
}

#[test]
fn check_script_agrees_with_bt_cpp() {
    let data = include_str!("script_oracle.tsv");
    let mut cases = 0;
    let mut failures = Vec::new();
    for line in data.lines().filter(|l| !l.starts_with('#')) {
        let (hex, result) = line.split_once('\t').expect("a tab");
        let script = String::from_utf8(unhex(hex)).expect("UTF-8 script");
        // The C++ message stops at a NUL byte (`what()` is a C string). XML
        // cannot contain NUL, so these scripts do not matter.
        if script.contains('\0') {
            continue;
        }
        let expected = String::from_utf8_lossy(&unhex(result)).into_owned();
        let actual = match check_script(&script) {
            Ok(_) => "OK".to_owned(),
            Err(err) => format!("ERR {}", err.message),
        };
        if actual != expected {
            failures.push(format!(
                "{script:?}\n  BT.CPP:   {expected}\n  FastTick: {actual}"
            ));
        }
        cases += 1;
    }
    assert!(cases > 3500, "{cases}");
    assert!(
        failures.is_empty(),
        "{} of {cases} differ:\n{}",
        failures.len(),
        failures
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
