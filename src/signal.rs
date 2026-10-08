//! Connection to the Signal servers through presage, as a linked (secondary) device.

use futures::channel::oneshot;
use presage::Manager;
use presage::libsignal_service::configuration::SignalServers;
use presage::manager::Registered;
use presage::model::identity::OnNewIdentity;
use presage_store_sqlite::{SqliteStore, SqliteStoreError};
use url::Url;

pub type SignalError = presage::Error<SqliteStoreError>;

/// Opens (creating it if needed) the local SQLCipher store holding the device keys.
pub async fn open_store(path: &str, passphrase: Option<&str>) -> Result<SqliteStore, SqliteStoreError> {
    SqliteStore::open_with_passphrase(path, passphrase, OnNewIdentity::Trust).await
}

/// Deletes the local store (database plus its SQLite `-wal`/`-shm` files), forgetting the link.
///
/// The device stays listed on the phone (Settings > Linked devices) until it is removed there.
pub fn remove_store(path: &str) -> std::io::Result<()> {
    for file in [path.to_string(), format!("{path}-wal"), format!("{path}-shm")] {
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// Links this client as a secondary device of an existing Signal account.
///
/// `provisioning_link` receives the `sgnl://linkdevice?...` URL as soon as the Signal
/// servers hand it out; it must be shown to the user (usually as a QR code) and scanned
/// from the primary phone. The returned future resolves once the phone confirms.
pub async fn link_device(
    store: SqliteStore,
    device_name: &str,
    provisioning_link: oneshot::Sender<Url>,
) -> Result<Manager<SqliteStore, Registered>, SignalError> {
    Manager::link_secondary_device(store, SignalServers::Production, device_name.to_string(), provisioning_link)
        .await
}

/// Loads an already linked device from the store.
pub async fn load_registered(store: SqliteStore) -> Result<Manager<SqliteStore, Registered>, SignalError> {
    Manager::load_registered(store).await
}
