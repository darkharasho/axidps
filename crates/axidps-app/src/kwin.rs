//! The KWin rule that keeps the overlay host's window out of sight on KDE. Wayland ignores
//! "start hidden", so its GL window would otherwise sit on the taskbar, the pager and Alt+Tab.
//! It is the rule `apps/axidps-overlay/install.sh` writes. Other desktops (GNOME) get no rule.

use std::path::Path;
use std::process::{Command, Stdio};

const RULE: &str = "axidps-overlay-hidden";
const FILE: &str = "kwinrulesrc";

/// The rule's keys and values, as install.sh writes them.
fn rule_keys() -> Vec<(&'static str, &'static str)> {
    let mut keys = vec![
        ("Description", "axidps overlay host: hidden"),
        ("wmclass", "axidps-overlay"),
        ("wmclassmatch", "1"),
    ];
    for (key, rule) in [
        ("skiptaskbar", "skiptaskbarrule"),
        ("skippager", "skippagerrule"),
        ("skipswitcher", "skipswitcherrule"),
        ("minimize", "minimizerule"),
    ] {
        keys.push((key, "true"));
        // 2: "apply initially".
        keys.push((rule, "2"));
    }
    keys
}

/// `order` (the General group's comma-separated rule list) with the rule appended, or `None`
/// when it is already listed.
fn order_with_rule(order: &str) -> Option<String> {
    if order.split(',').any(|r| r.trim() == RULE) {
        return None;
    }
    Some(if order.is_empty() {
        RULE.to_owned()
    } else {
        format!("{order},{RULE}")
    })
}

/// Runs a program with arguments; its trimmed stdout when it succeeded.
type Run<'a> = &'a mut dyn FnMut(&str, &[&str]) -> Option<String>;

fn read(run: Run, group: &str, key: &str) -> Option<String> {
    run(
        "kreadconfig6",
        &["--file", FILE, "--group", group, "--key", key],
    )
}

fn write(run: Run, group: &str, key: &str, value: &str) -> bool {
    run(
        "kwriteconfig6",
        &["--file", FILE, "--group", group, "--key", key, value],
    )
    .is_some()
}

/// Writes the rule when any of it is missing or different, then asks KWin to reload its rules.
/// True when it wrote anything. A rule that is already right is left alone (KWin is not asked).
fn ensure_with(run: Run) -> bool {
    let keys = rule_keys();
    let order = read(run, "General", "Order").unwrap_or_default();
    let new_order = order_with_rule(&order);
    let stale: Vec<_> = keys
        .iter()
        .filter(|(k, v)| read(run, RULE, k).as_deref() != Some(*v))
        .collect();
    if stale.is_empty() && new_order.is_none() {
        return false;
    }
    for (k, v) in stale {
        if !write(run, RULE, k, v) {
            eprintln!("axidps: cannot write the KWin rule ({k})");
            return true;
        }
    }
    if let Some(order) = new_order
        && !write(run, "General", "Order", &order)
    {
        eprintln!("axidps: cannot add the KWin rule to the rule order");
        return true;
    }
    let reconfigured = run(
        "dbus-send",
        &[
            "--session",
            "--type=method_call",
            "--dest=org.kde.KWin",
            "/KWin",
            "org.kde.KWin.reconfigure",
        ],
    )
    .is_some()
        || run(
            "gdbus",
            &[
                "call",
                "--session",
                "--dest",
                "org.kde.KWin",
                "--object-path",
                "/KWin",
                "--method",
                "org.kde.KWin.reconfigure",
            ],
        )
        .is_some();
    if !reconfigured {
        eprintln!("axidps: wrote the KWin rule; KWin applies it at the next login");
    }
    true
}

/// Whether `program` is an executable file in a `PATH` folder.
fn on_path(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        Path::new(&dir)
            .join(program)
            .metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

fn run_program(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Installs the rule on KDE (when `kwriteconfig6` is available); does nothing elsewhere.
pub fn ensure() {
    if !on_path("kwriteconfig6") || !on_path("kreadconfig6") {
        return;
    }
    if ensure_with(&mut run_program) {
        eprintln!("axidps: KWin rule {RULE} installed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A fake kreadconfig6/kwriteconfig6 over a map, recording every call.
    struct Fake {
        config: HashMap<(String, String), String>,
        calls: Vec<String>,
    }

    impl Fake {
        fn new() -> Fake {
            Fake {
                config: HashMap::new(),
                calls: Vec::new(),
            }
        }

        fn run(&mut self, program: &str, args: &[&str]) -> Option<String> {
            self.calls.push(program.to_owned());
            let (group, key) = (args.get(3)?, args.get(5)?);
            let slot = (group.to_string(), key.to_string());
            match program {
                "kreadconfig6" => Some(self.config.get(&slot).cloned().unwrap_or_default()),
                "kwriteconfig6" => {
                    self.config.insert(slot, args.get(6)?.to_string());
                    Some(String::new())
                }
                _ => None,
            }
        }

        fn get(&self, group: &str, key: &str) -> Option<&str> {
            self.config
                .get(&(group.to_owned(), key.to_owned()))
                .map(String::as_str)
        }
    }

    fn ensure(fake: &mut Fake) -> bool {
        ensure_with(&mut |p, a| fake.run(p, a))
    }

    #[test]
    fn rule_matches_install_sh() {
        let mut fake = Fake::new();
        assert!(ensure(&mut fake));
        assert_eq!(fake.get(RULE, "wmclass"), Some("axidps-overlay"));
        assert_eq!(fake.get(RULE, "wmclassmatch"), Some("1"));
        for k in ["skiptaskbar", "skippager", "skipswitcher", "minimize"] {
            assert_eq!(fake.get(RULE, k), Some("true"), "{k}");
            assert_eq!(fake.get(RULE, &format!("{k}rule")), Some("2"), "{k}rule");
        }
        assert_eq!(fake.get("General", "Order"), Some(RULE));
        assert!(fake.calls.iter().any(|c| c == "dbus-send"));
    }

    #[test]
    fn second_run_changes_nothing_and_does_not_reconfigure() {
        let mut fake = Fake::new();
        ensure(&mut fake);
        fake.calls.clear();
        assert!(!ensure(&mut fake));
        assert!(fake.calls.iter().all(|c| c == "kreadconfig6"));
    }

    #[test]
    fn existing_rules_are_kept_in_the_order() {
        let mut fake = Fake::new();
        fake.config
            .insert(("General".into(), "Order".into()), "a,b".into());
        ensure(&mut fake);
        assert_eq!(
            fake.get("General", "Order"),
            Some("a,b,axidps-overlay-hidden")
        );
    }

    #[test]
    fn order_is_not_duplicated() {
        assert_eq!(order_with_rule(""), Some(RULE.to_owned()));
        assert_eq!(order_with_rule("x"), Some(format!("x,{RULE}")));
        assert_eq!(order_with_rule(&format!("x,{RULE},y")), None);
    }

    #[test]
    fn a_changed_key_is_rewritten() {
        let mut fake = Fake::new();
        ensure(&mut fake);
        fake.config
            .insert((RULE.into(), "skiptaskbar".into()), "false".into());
        assert!(ensure(&mut fake));
        assert_eq!(fake.get(RULE, "skiptaskbar"), Some("true"));
    }
}
