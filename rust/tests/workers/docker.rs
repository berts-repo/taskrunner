//! The Docker runner against a real Docker daemon. Gated: set
//! TASKRUNNER_LIVE_DOCKER=1 with Docker running and the worker images built
//! (`npm run build:images`). The runner has no other automated exercise.

use std::sync::{Arc, Mutex};

use taskrunner::config::ResourceLimits;
use taskrunner::workers::runner::{
    DockerRunner, DockerRunnerOptions, EgressDecision, WorkerRunner, WorkerSpawnSpec,
};
use tokio::io::AsyncReadExt;

fn live() -> bool {
    std::env::var("TASKRUNNER_LIVE_DOCKER").as_deref() == Ok("1")
}

#[tokio::test]
async fn runs_a_command_in_a_worker_image_behind_the_proxy_and_cleans_up() {
    if !live() {
        eprintln!("skipped: TASKRUNNER_LIVE_DOCKER is not set");
        return;
    }
    let workspace = tempfile::tempdir().unwrap();
    let decisions: Arc<Mutex<Vec<EgressDecision>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = decisions.clone();
    let scope = format!("test-{}", std::process::id());
    let runner = DockerRunner::new(DockerRunnerOptions {
        workspace_dir: workspace.path().to_path_buf(),
        scope_id: scope.clone(),
        image: "taskrunner/codex-worker".into(),
        auth_volume: None,
        auth_mounts: vec![],
        proxy_image: "taskrunner/egress-proxy".into(),
        allowed_domains: vec!["example.com".into()],
        limits: ResourceLimits::default(),
        on_egress: Some(Arc::new(move |d| sink.lock().unwrap().push(d))),
        docker_command: "docker".into(),
    });

    // One allowed and one refused connection, then a plain echo.
    let script = "curl -s -o /dev/null -m 5 http://example.com/ ; curl -s -o /dev/null -m 5 http://neverallowed.invalid/ ; echo hi";
    let spec = WorkerSpawnSpec {
        argv: vec!["sh".into(), "-c".into(), script.into()],
        env: Default::default(),
    };
    let mut worker = runner.start(spec).await.unwrap();
    let mut stdout = String::new();
    worker.child.stdout.take().unwrap().read_to_string(&mut stdout).await.unwrap();
    let status = worker.child.wait().await.unwrap();
    assert!(status.success());
    assert!(stdout.contains("hi"));
    runner.dispose().await;

    let seen = decisions.lock().unwrap();
    assert!(seen.iter().any(|d| d.allowed && d.host == "example.com"), "{seen:?}");
    assert!(seen.iter().any(|d| !d.allowed && d.host == "neverallowed.invalid"), "{seen:?}");

    // Nothing of this turn is left behind.
    for (kind, name) in [
        ("container", format!("taskrunner-worker-{scope}")),
        ("container", format!("taskrunner-proxy-{scope}")),
        ("network", format!("taskrunner-egress-{scope}")),
    ] {
        let inspect =
            std::process::Command::new("docker").args([kind, "inspect", &name]).output().unwrap();
        assert!(!inspect.status.success(), "{kind} {name} still exists");
    }
}
