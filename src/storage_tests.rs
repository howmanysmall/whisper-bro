#![allow(clippy::unwrap_used)]

use std::fs;

use super::Paths;

fn paths(root: &std::path::Path) -> Paths {
    Paths {
        config: root.join("config/config.toml"),
        data: root.join("data"),
    }
}

#[test]
fn copies_missing_files() {
    let (old_dir, new_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (old, new) = (paths(old_dir.path()), paths(new_dir.path()));
    fs::create_dir_all(old.config.parent().unwrap()).unwrap();
    fs::create_dir_all(&old.data).unwrap();
    fs::write(&old.config, "old config").unwrap();
    fs::write(old.recovery(), "old dictation").unwrap();

    new.migrate_from(&old).unwrap();

    assert_eq!(fs::read_to_string(&new.config).unwrap(), "old config");
    assert_eq!(fs::read_to_string(new.recovery()).unwrap(), "old dictation");
    assert!(old.config.exists());
}

#[test]
fn never_overwrites_existing_files() {
    let (old_dir, new_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (old, new) = (paths(old_dir.path()), paths(new_dir.path()));
    for p in [&old, &new] {
        fs::create_dir_all(p.config.parent().unwrap()).unwrap();
        fs::create_dir_all(&p.data).unwrap();
    }
    fs::write(&old.config, "old config").unwrap();
    fs::write(old.recovery(), "old dictation").unwrap();
    fs::write(&new.config, "new config").unwrap();
    fs::write(new.recovery(), "new dictation").unwrap();

    new.migrate_from(&old).unwrap();

    assert_eq!(fs::read_to_string(&new.config).unwrap(), "new config");
    assert_eq!(fs::read_to_string(new.recovery()).unwrap(), "new dictation");
}

#[test]
fn creates_nothing_when_old_is_empty() {
    let (old_dir, new_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (old, new) = (paths(old_dir.path()), paths(new_dir.path()));

    new.migrate_from(&old).unwrap();

    assert_eq!(fs::read_dir(new_dir.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn migrated_files_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let (old_dir, new_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (old, new) = (paths(old_dir.path()), paths(new_dir.path()));
    fs::create_dir_all(old.config.parent().unwrap()).unwrap();
    fs::write(&old.config, "old config").unwrap();

    new.migrate_from(&old).unwrap();

    let mode = fs::metadata(&new.config).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}
