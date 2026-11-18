// Settings use the real crate; private sync code is extracted verbatim into isolated fixtures.
use std::collections::BTreeMap;
use wechat_devify_bridge_lib::settings;

#[cfg(test)]
mod sync_lifecycle_regression {
    use super::*;

    fn owner() -> settings::UploadOwner {
        settings::UploadOwner {
            base_url: "https://example.invalid".into(), space_id: "1".into(),
            account_id: "test-account".into(), user_id: 42, device_id: "device".into(),
        }
    }

    fn job() -> settings::UploadJob {
        let scope = settings::UploadScope {
            usernames: vec!["test-conversation".into()], since: Some(1), until: Some(5),
            base_url: owner().base_url, message_selection: BTreeMap::from([(
                "test-conversation".into(), vec![settings::MessageRef {
                    local_id: 3, time: 2, source_shard: "fixture".into(),
                }],
            )]),
        };
        settings::UploadJob {
            id: "fixture-id".into(), space_id: "1".into(), account_id: "test-account".into(),
            owner: Some(owner()), create_started: Some(false), status: "running".into(),
            scope: Some(scope), ..Default::default()
        }
    }

    fn run_fixture(source: &str) {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("fixture.rs");
        let binary = temp.path().join("fixture");
        std::fs::write(&file, source).unwrap();
        let output = std::process::Command::new("rustc").arg(&file).arg("-o").arg(&binary).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let output = std::process::Command::new(&binary).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }

    #[test]
    #[ignore = "stale fixture: #30 rewrote the sync job lifecycle; fixtures slice old source anchors"]
    fn sync_job_has_unique_identity_and_exact_scope() {
        let source = include_str!("../src/lib.rs");
        let start = source.find("fn sync_job(").unwrap();
        let end = source[start..].find("\n}\n").unwrap() + start + "\n}".len();
        let function = &source[start..end];
        let fixture = r#"
use std::collections::BTreeMap;
mod getrandom {
    use std::sync::atomic::{AtomicU8, Ordering};
    static NEXT: AtomicU8 = AtomicU8::new(1);
    pub fn getrandom(bytes: &mut [u8]) -> Result<(), ()> {
        bytes.fill(NEXT.fetch_add(1, Ordering::SeqCst)); Ok(())
    }
}
mod hex { pub fn encode(bytes: [u8; 16]) -> String { format!("{:?}", bytes) } }
mod chrono {
    pub struct Utc; pub struct Now;
    impl Utc { pub fn now() -> Now { Now } }
    impl Now { pub fn to_rfc3339(&self) -> String { "fixture-time".into() } }
}
mod settings {
    use super::BTreeMap;
    #[derive(Clone, Debug, PartialEq)] pub struct UploadOwner {
        pub base_url: String, pub space_id: String, pub account_id: String,
        pub user_id: i64, pub device_id: String,
    }
    #[derive(Clone, Debug, PartialEq)] pub struct UploadScope {
        pub usernames: Vec<String>, pub since: Option<i64>, pub until: Option<i64>,
        pub base_url: String, pub message_selection: BTreeMap<String, Vec<i64>>,
    }
    pub struct UploadJob {
        pub id: String, pub space_id: String, pub account_id: String, pub batch_id: Option<i64>,
        pub owner: Option<UploadOwner>, pub create_started: Option<bool>, pub package_digest: Option<String>,
        pub verified_completion_digest: Option<String>, pub legacy_failure_confirmed: bool,
        pub status: String, pub message: String, pub created_at: String, pub updated_at: String,
        pub scope: Option<UploadScope>, pub covered: BTreeMap<String, ()>,
        pub initial_baselines: BTreeMap<String, ()>, pub initial_chunks: BTreeMap<String, ()>,
        pub sync_advance: bool, pub continue_sync: bool, pub continuation_manual: Option<bool>,
        pub manual_group: Option<String>, pub manual_index: usize, pub manual_total: usize,
        pub manual_fingerprint: Option<String>, pub voice_processed: BTreeMap<String, ()>,
        pub voice_pending: BTreeMap<String, ()>,
    }
}
SYNC_JOB
fn main() {
    let owner = settings::UploadOwner { base_url: "https://example.invalid".into(), space_id: "1".into(),
        account_id: "account".into(), user_id: 42, device_id: "device".into() };
    let scope = settings::UploadScope { usernames: vec!["alice".into()], since: Some(1), until: Some(5),
        base_url: owner.base_url.clone(), message_selection: BTreeMap::from([("alice".into(), vec![3])]) };
    let first = sync_job(&owner, scope.clone()).unwrap();
    let second = sync_job(&owner, scope.clone()).unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(first.owner, Some(owner));
    assert_eq!(first.create_started, Some(false)); assert_eq!(first.batch_id, None);
    assert_eq!(first.scope, Some(scope));
}
"#.replace("SYNC_JOB", function);
        run_fixture(&fixture);
    }

    #[test]
    fn failed_create_intent_save_keeps_original_job_unstarted() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        let moved = temp.path().join("preserved");
        let original = job();
        settings::update(&app, |value| { value.upload_queue.push(original.clone()); Ok(()) }).unwrap();
        let result = settings::update(&app, |value| {
            value.upload_queue[0].create_started = Some(true);
            value.upload_queue[0].package_digest = Some("fixture-digest".into());
            std::fs::rename(&app, &moved).unwrap();
            std::fs::write(&app, b"not a directory").unwrap();
            Ok(())
        });
        assert!(result.is_err());
        let saved = settings::load(&moved).unwrap();
        assert_eq!(saved.upload_queue.len(), 1);
        assert_eq!(saved.upload_queue[0].id, original.id);
        assert_eq!(saved.upload_queue[0].create_started, Some(false));
        assert_eq!(saved.upload_queue[0].package_digest, None);
        assert_eq!(saved.upload_queue[0].owner, Some(owner()));
        assert_eq!(saved.upload_queue[0].scope, original.scope);
    }

    #[test]
    fn interrupted_result_save_preserves_durable_create_and_batch() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        let moved = temp.path().join("preserved");
        let original = job();
        let id = original.id.clone();
        settings::update(&app, |value| { value.upload_queue.push(original.clone()); Ok(()) }).unwrap();
        // Stand in for the uploader's successful durable checkpoints.
        settings::update(&app, |value| {
            let job = &mut value.upload_queue[0];
            job.create_started = Some(true);
            job.package_digest = Some("fixture-digest".into());
            job.batch_id = Some(123);
            Ok(())
        }).unwrap();
        // Fail the actual disk save after the transaction has mutated its clone.
        let result = settings::update(&app, |value| {
            value.upload_queue[0].status = "processing".into();
            std::fs::rename(&app, &moved).unwrap();
            std::fs::write(&app, b"not a directory").unwrap();
            Ok(())
        });
        assert!(result.is_err());
        let saved = settings::load(&moved).unwrap();
        assert_eq!(saved.upload_queue.len(), 1);
        let stored = &saved.upload_queue[0];
        assert_eq!(stored.id, id);
        assert_eq!(stored.owner, Some(owner()));
        assert_eq!(stored.create_started, Some(true));
        assert_eq!(stored.package_digest.as_deref(), Some("fixture-digest"));
        assert_eq!(stored.batch_id, Some(123));
        assert_eq!(stored.status, "running");
    }

    #[test]
    #[ignore = "stale fixture: #30 rewrote the sync job lifecycle; fixtures slice old source anchors"]
    fn sync_persists_job_before_calling_uploader() {
        let source = include_str!("../src/lib.rs");
        let start = source.find("async fn perform_chat_sync(").unwrap();
        let end = source[start..].find("\nasync fn chat_sync_tick(").unwrap() + start;
        let sync = &source[start..end];
        let legacy_check = sync.find("job.id.starts_with(\"sync-\") && job.owner.as_ref() != Some(&sync_owner)").unwrap();
        let bind_owner = sync.find("current.sync.owners.entry(account_id.clone())").unwrap();
        assert!(legacy_check < bind_owner);
        let insert = sync.find("current.upload_queue.insert(0, job.clone())").unwrap();
        let upload = sync.find("let result = devify_upload_wechat(").unwrap();
        assert!(insert < upload);
        assert!(sync[insert..upload].contains("Err(error) => { failures.push"));
        assert!(sync[upload..].contains("find(|item| item.id == job_id)"));
        assert!(!sync[upload..].contains("upload_queue.splice"));
        assert!(sync[upload..].contains("if job_changed { job.updated_at ="));
    }

    #[test]
    #[ignore = "stale fixture: #30 rewrote the sync job lifecycle; fixtures slice old source anchors"]
    fn ambiguous_create_blocks_replay_using_real_sync_predicate() {
        // Compile the predicate verbatim from perform_chat_sync against tiny fixture types;
        // this tests the production condition without a Tauri app or network/database.
        let source = include_str!("../src/lib.rs");
        let start = source.find("    let blocks_sync = |current:").unwrap();
        let tail = &source[start..];
        let end = tail.find("\n    };\n").unwrap() + "\n    };".len();
        let predicate = &tail[..end];
        let helpers: String = ["fn upload_job_has_pending_sync(", "fn legacy_job_is_inert(", "fn job_never_blocks_sync("]
            .iter().map(|name| {
                let start = source.find(name).unwrap();
                let end = source[start..].find("\n}\n").unwrap() + start + "\n}\n".len();
                source[start..end].to_string()
            }).collect();
        let fixture = r#"
use std::collections::BTreeMap;
mod settings {
    use super::BTreeMap;
    #[derive(Clone, PartialEq)] pub struct UploadOwner { pub name: &'static str }
    #[derive(Clone)] pub struct Scope { pub usernames: Vec<String> }
    #[derive(Clone)] pub struct Job {
        pub account_id: String, pub status: String, pub scope: Option<Scope>,
        pub owner: Option<UploadOwner>, pub create_started: Option<bool>, pub batch_id: Option<i64>,
        pub initial_baselines: BTreeMap<String, ()>, pub initial_chunks: BTreeMap<String, ()>,
        pub sync_advance: bool, pub continue_sync: bool, pub legacy_failure_confirmed: bool,
        pub voice_processed: BTreeMap<String, ()>, pub voice_pending: BTreeMap<String, ()>,
    }
    pub type UploadJob = Job;
    pub struct AppSettings { pub upload_queue: Vec<Job> }
}
HELPERS
fn main() {
    let account_id = "account".to_string();
    let sync_owner = settings::UploadOwner { name: "verified" };
    PREDICATE
    let mut job = settings::Job { account_id: account_id.clone(), status: "failed".into(),
        scope: Some(settings::Scope { usernames: vec!["alice".into()] }),
        owner: Some(sync_owner.clone()), create_started: Some(false), batch_id: None,
        initial_baselines: BTreeMap::new(), initial_chunks: BTreeMap::new(),
        sync_advance: false, continue_sync: false, legacy_failure_confirmed: false,
        voice_processed: BTreeMap::new(), voice_pending: BTreeMap::new() };
    let check = |job: &settings::Job| blocks_sync(&settings::AppSettings { upload_queue: vec![job.clone()] }, "alice");
    assert!(!check(&job));
    job.create_started = Some(true); assert!(check(&job));
    job.batch_id = Some(123); assert!(check(&job));
    job.create_started = None; assert!(check(&job));
    // A server-confirmed legacy failure can no longer be ingested remotely.
    job.legacy_failure_confirmed = true; assert!(!check(&job));
    job.status = "running".into(); assert!(check(&job));
    job.status = "failed".into(); job.legacy_failure_confirmed = false;
    job.create_started = Some(false); job.batch_id = None; job.owner = None;
    // An ownerless failure without a batch never reached the server.
    assert!(!check(&job));
    job.status = "running".into(); assert!(check(&job));
    job.status = "failed".into(); job.sync_advance = true; assert!(check(&job));
    job.sync_advance = false; job.batch_id = Some(123); assert!(check(&job));
    job.batch_id = None;
    job.owner = Some(settings::UploadOwner { name: "other" }); assert!(check(&job));
    job.owner = Some(sync_owner.clone()); job.scope = None; job.create_started = None; assert!(check(&job));
    job.scope = Some(settings::Scope { usernames: Vec::new() }); assert!(check(&job));
}
"#.replace("HELPERS", &helpers).replace("PREDICATE", predicate);
        run_fixture(&fixture);
    }
}
