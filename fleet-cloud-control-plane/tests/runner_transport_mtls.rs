mod common;

use fleet_cloud_control_plane::runner_gateway::{connection, registry};
use fleet_cloud_runner::{identity, journal::CommandJournal, outbox::EventOutbox, transport};
use fleet_cloud_wire::event::RunnerEvent;
use fleet_cloud_wire::runner::{ClientHello, CommandAckStatus, RunnerCapability};
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

struct Certificates {
    ca_pem: String,
    server_der: CertificateDer<'static>,
    server_key: PrivateKeyDer<'static>,
    client_pem: String,
    client_key_pem: String,
    client_fingerprint: Vec<u8>,
    ca_der: CertificateDer<'static>,
}

fn certificates() -> Certificates {
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_key = KeyPair::generate().unwrap();
    let ca = ca_params.self_signed(&ca_key).unwrap();

    let server_key = KeyPair::generate().unwrap();
    let server = CertificateParams::new(vec!["localhost".into()])
        .unwrap()
        .signed_by(&server_key, &ca, &ca_key)
        .unwrap();
    let client_key = KeyPair::generate().unwrap();
    let client = CertificateParams::new(vec!["runner-test".into()])
        .unwrap()
        .signed_by(&client_key, &ca, &ca_key)
        .unwrap();
    Certificates {
        ca_pem: ca.pem(),
        server_der: server.der().clone(),
        server_key: PrivatePkcs8KeyDer::from(server_key.serialize_der()).into(),
        client_pem: client.pem(),
        client_key_pem: client_key.serialize_pem(),
        client_fingerprint: Sha256::digest(client.der().as_ref()).to_vec(),
        ca_der: ca.der().clone(),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn mtls_runner_persists_command_before_ack_and_uploads_outbox(pool: sqlx::PgPool) {
    let certificates = certificates();
    common::seed(&pool).await;
    sqlx::query("INSERT INTO runner_pools(id,organization_id,project_id,name) VALUES('pool_test','org_test','proj_test','Test')")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO runners(id,organization_id,project_id,pool_id,name,certificate_fingerprint) VALUES('runner_test','org_test','proj_test','pool_test','Runner',$1)")
        .bind(&certificates.client_fingerprint).execute(&pool).await.unwrap();
    let created = common::post_json(
        common::router(pool.clone()),
        "/tasks",
        common::VALID_KEY,
        Some("mtls-task-001"),
        common::create_task_body("proj_test", "mTLS task"),
    )
    .await
    .body;
    let task_id = created["task"]["id"].as_str().unwrap().to_owned();
    let run_id = created["run"]["id"].as_str().unwrap().to_owned();
    let command_id: String = sqlx::query_scalar("SELECT id FROM commands WHERE task_id=$1")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    let hello = ClientHello {
        protocol_version: fleet_cloud_wire::RUNNER_PROTOCOL_VERSION,
        runner_id: "runner_test".into(),
        build_version: "e2e".into(),
        platform: "linux".into(),
        architecture: "x86_64".into(),
        max_concurrency: 1,
        capabilities: vec![RunnerCapability {
            name: "claude_code".into(),
            version: 1,
        }],
        last_cloud_cursor: None,
        outbox_first_sequence: None,
        outbox_last_sequence: None,
    };
    registry::connect(&pool, &hello, &certificates.client_fingerprint)
        .await
        .unwrap();
    registry::assign_command(
        &pool,
        "runner_test",
        &command_id,
        Some(&hello.capabilities[0]),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE runners SET status='offline' WHERE id='runner_test'")
        .execute(&pool)
        .await
        .unwrap();

    let directory = tempfile::tempdir().unwrap();
    let mut journal = CommandJournal::open(&directory.path().join("journal.sqlite")).unwrap();
    let outbox = EventOutbox::open(&directory.path().join("outbox.sqlite")).unwrap();
    outbox
        .append(RunnerEvent {
            source_event_id: "mtls-source-1".into(),
            sequence: 0,
            event_type: "run.output".into(),
            occurred_at: chrono::Utc::now(),
            data: serde_json::json!({"task_id":task_id,"run_id":run_id,"text":"hello"}),
            schema_version: 1,
        })
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let acceptor = connection::tls_acceptor(
        vec![certificates.server_der],
        certificates.server_key,
        certificates.ca_der,
    )
    .unwrap();
    let server = tokio::spawn(connection::serve_one(listener, acceptor, pool.clone()));
    let tls = identity::client_config(
        certificates.ca_pem.as_bytes(),
        certificates.client_pem.as_bytes(),
        certificates.client_key_pem.as_bytes(),
    )
    .unwrap();
    let url = format!("wss://localhost:{port}");
    {
        let client = transport::run_once(&url, tls, hello, &mut journal, &outbox);
        tokio::pin!(client);
        let observed = async {
            for _ in 0..100 {
                let status: Option<String> =
                    sqlx::query_scalar("SELECT status FROM commands WHERE id=$1")
                        .bind(&command_id)
                        .fetch_optional(&pool)
                        .await
                        .unwrap();
                let events = sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM runner_source_events WHERE runner_id='runner_test'",
                )
                .fetch_one(&pool)
                .await
                .unwrap();
                if status.as_deref() == Some("accepted") && events == 1 {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            panic!("mTLS command/event exchange did not complete");
        };
        tokio::select! {
            result=&mut client=>panic!("Runner disconnected early: {result:?}"),
            _=observed=>{}
        }
    }
    assert_eq!(
        journal.ack_status(&command_id).unwrap(),
        CommandAckStatus::Accepted
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(outbox.range().unwrap(), (None, None));
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), server).await;
}
