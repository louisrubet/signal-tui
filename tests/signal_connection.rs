//! Tests against the real Signal servers. They need network access, so they are ignored by default:
//!
//!   cargo test --test signal_connection -- --ignored --nocapture
//!
//! `provisioning_link` only checks that the servers answer: no account is involved.
//! To really link this client to your account, see `examples/link_device.rs`.

use std::time::Duration;

use futures::channel::oneshot;
use futures::future::{self, Either};
use signal_tui::signal;

const TIMEOUT: Duration = Duration::from_secs(30);

#[tokio::test]
#[ignore = "requires network access to the Signal servers"]
async fn provisioning_link() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("store.db3");
    let store = signal::open_store(db.to_str().unwrap(), None).await.unwrap();

    let (tx, rx) = oneshot::channel();
    let linking = Box::pin(signal::link_device(store, "signal-tui test", tx));

    // Getting the provisioning URL proves the websocket to the servers is up.
    // Nobody scans it, so `linking` is dropped (cancelled) once we have it.
    let url = match tokio::time::timeout(TIMEOUT, future::select(linking, rx)).await {
        Err(_) => panic!("no provisioning link from the Signal servers after {TIMEOUT:?}"),
        Ok(Either::Left((result, _))) => panic!("linking ended before the link was sent: {:?}", result.err()),
        Ok(Either::Right((url, _))) => url.expect("provisioning link channel closed"),
    };

    assert_eq!(url.scheme(), "sgnl");
    assert_eq!(url.host_str(), Some("linkdevice"));
    let params: Vec<String> = url.query_pairs().map(|(k, _)| k.into_owned()).collect();
    assert!(params.iter().any(|k| k == "uuid"), "missing uuid in {url}");
    assert!(params.iter().any(|k| k == "pub_key"), "missing pub_key in {url}");
}

