use std::fs;
#[path = "common/xet_github_mock.rs"]
mod mock;
use mock::Server;
fn path(value: impl AsRef<std::path::Path>) -> camino::Utf8PathBuf {
    camino::Utf8PathBuf::from_path_buf(value.as_ref().to_owned()).unwrap()
}
#[test]
fn actual_xet_github_whole_store_round_trip_retains_prior_export_on_failure() {
    let program = std::env::var_os("PROJCLI_TEST_XET_ENGINE");
    assert!(program.is_some() || std::env::var_os("PROJCLI_TEST_XET_REQUIRED").is_none());
    let Some(program) = program else { return };
    let engine = proj::xet::Engine::new(program).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = path(temp.path());
    let project = root.join("project");
    let source = project.join("raw");
    let store = root.join("store");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("data.csv"), b"original bytes\n").unwrap();
    let first = proj::xet::publish(
        &project,
        &store,
        &[("raw-data".into(), source.clone())],
        "v1.0.0",
        &engine,
    )
    .unwrap();
    let server = Server::start();
    let client = proj::github_release::GithubReleaseClient::new(
        "owner/repo",
        &server.base,
        Some("fixture-token".into()),
    )
    .unwrap();
    let reference = proj::xet_github::upload(
        &project,
        &store,
        &client,
        "data",
        true,
        64 * 1024 * 1024,
        &engine,
    )
    .unwrap();
    assert_eq!(reference.structure, "xet");
    assert!(reference.snapshots.contains(&first));
    let prior = {
        let state = server.state.lock().unwrap();
        assert_eq!(state.uploads.len(), 1);
        state.assets.clone()
    };
    fs::remove_dir_all(&store).unwrap();
    fs::remove_dir_all(&source).unwrap();
    let imported = proj::xet_github::download(
        &project,
        &client,
        &reference,
        64 * 1024 * 1024,
        64 * 1024 * 1024,
        &engine,
    )
    .unwrap();
    let restored = project.join("restored");
    proj::xet::restore(
        &project,
        imported.path(),
        &first,
        "raw-data",
        &restored,
        true,
        &engine,
    )
    .unwrap();
    assert_eq!(
        fs::read(restored.join("data.csv")).unwrap(),
        b"original bytes\n"
    );
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("data.csv"), b"updated bytes\n").unwrap();
    let second = proj::xet::publish(
        &project,
        imported.path(),
        &[("raw-data".into(), source)],
        "v1.0.1",
        &engine,
    )
    .unwrap();
    server.state.lock().unwrap().fail_xet = true;
    assert!(
        proj::xet_github::upload(
            &project,
            imported.path(),
            &client,
            "data",
            true,
            64 * 1024 * 1024,
            &engine
        )
        .is_err()
    );
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.assets, prior);
        assert!(state.deleted.is_empty());
    }
    {
        let mut state = server.state.lock().unwrap();
        state.fail_xet = false;
        state.disconnect_xet = true;
    }
    let uncertain = proj::xet_github::upload(
        &project,
        imported.path(),
        &client,
        "data",
        true,
        64 * 1024 * 1024,
        &engine,
    )
    .unwrap_err();
    assert!(format!("{uncertain:#}").contains("uncertain"));
    {
        let mut state = server.state.lock().unwrap();
        state.disconnect_xet = false;
        assert!(state.deleted.is_empty());
        for (name, value) in &prior {
            assert_eq!(&state.assets[name], value);
        }
    }
    // Return corrupt downloadable bytes after accepting the unique new upload.
    server.state.lock().unwrap().corrupt = true;
    assert!(
        proj::xet_github::upload(
            &project,
            imported.path(),
            &client,
            "data",
            true,
            64 * 1024 * 1024,
            &engine
        )
        .is_err()
    );
    {
        let state = server.state.lock().unwrap();
        assert!(state.deleted.is_empty());
        for (name, value) in &prior {
            assert_eq!(&state.assets[name], value);
        }
    }
    assert_eq!(
        fs::read(restored.join("data.csv")).unwrap(),
        b"original bytes\n"
    );
    server.state.lock().unwrap().corrupt = false;
    // Retry uses the existing immutable new ZIP rather than deleting either version.
    let updated = proj::xet_github::upload(
        &project,
        imported.path(),
        &client,
        "data",
        true,
        64 * 1024 * 1024,
        &engine,
    )
    .unwrap();
    assert!(updated.snapshots.contains(&first) && updated.snapshots.contains(&second));
    let fresh = proj::xet_github::download(
        &project,
        &client,
        &updated,
        64 * 1024 * 1024,
        64 * 1024 * 1024,
        &engine,
    )
    .unwrap();
    proj::xet::restore(
        &project,
        fresh.path(),
        &second,
        "raw-data",
        &restored,
        true,
        &engine,
    )
    .unwrap();
    assert_eq!(
        fs::read(restored.join("data.csv")).unwrap(),
        b"updated bytes\n"
    );
    {
        let state = server.state.lock().unwrap();
        assert!(state.deleted.is_empty());
        assert_eq!(state.assets.len(), 2);
    }
    println!(
        "Mocked GitHub complete-store download/local-update/upload verified both snapshots and retained prior ZIP on failed download verification."
    );
    server.finish();
}
