use std::sync::OnceLock;
use tokio::sync::broadcast;

static SETTINGS_TX: OnceLock<broadcast::Sender<(String, String)>> = OnceLock::new();

fn get_tx() -> &'static broadcast::Sender<(String, String)> {
    SETTINGS_TX.get_or_init(|| {
        let (tx, _) = broadcast::channel(32);
        tx
    })
}

/// Subscribes to setting change notifications.
pub fn settings_subscribe() -> broadcast::Receiver<(String, String)> {
    get_tx().subscribe()
}

/// Broadcasts a setting change notification.
pub fn notify_setting_change(key: &str, value: &str) {
    let tx = get_tx();
    let _ = tx.send((key.to_string(), value.to_string()));
}
